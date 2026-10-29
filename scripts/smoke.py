#!/usr/bin/env python3
"""Exercise the compiled CLI, daemon, SSE, persistence, and real terminal UI."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import socket
import struct
import subprocess
import tempfile
import termios
import time
import urllib.request


def eventually(check, timeout=10):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, AssertionError, subprocess.SubprocessError) as error:
            last = error
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting for condition: {last}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="ochecore-smoke-") as directory:
        config = str(Path(directory) / "board.toml")
        base = [binary, "--config", config, "--port", str(port)]
        daemon = None

        def cli(*arguments, ok=True):
            result = subprocess.run(base + ["--json", *arguments], text=True, capture_output=True, timeout=5)
            assert (result.returncode == 0) == ok, (arguments, result.stdout, result.stderr)
            return json.loads(result.stdout if ok else result.stderr)

        def start(demo):
            process = subprocess.Popen(base + ["run"] + (["--demo"] if demo else []), stdout=subprocess.DEVNULL, stderr=log)
            eventually(lambda: cli("status"))
            return process

        with open(Path(directory) / "daemon.log", "w+") as log:
            try:
                assert "init" in cli("run", ok=False)["error"]
                assert "init" in cli(ok=False)["error"]
                assert not Path(config).exists()
                assert "interactive terminal" in cli("init", ok=False)["error"]
                original = cli("init", "--defaults")
                assert original["server"]["port"] == port
                assert "already exists" in cli("init", "--defaults", ok=False)["error"]
                assert "confirmation" in cli("config", "reset", "--defaults", ok=False)["error"]
                reset = cli("config", "reset", "--yes", "--defaults")
                assert reset["board"]["id"] != original["board"]["id"]
                assert cli("config", "validate")["board_id"] == reset["board"]["id"]

                # Exercise setup in a real terminal, including cancellation and EOF.
                def setup_terminal(arguments, answers, expected_success, config_path=config):
                    master, slave = pty.openpty()
                    process = subprocess.Popen([binary, "--config", config_path, "--port", str(port)] + arguments, stdin=slave, stdout=slave, stderr=slave)
                    output = bytearray()
                    try:
                        for prompt, answer in answers:
                            def prompted():
                                if select.select([master], [], [], 0.1)[0]:
                                    output.extend(os.read(master, 65536))
                                return prompt in output
                            eventually(prompted)
                            output.clear()
                            os.write(master, answer)
                        assert (process.wait(timeout=5) == 0) == expected_success
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait()
                        os.close(master)
                        os.close(slave)

                before_reset = Path(config).read_bytes()
                setup_terminal(["config", "reset"], [(b"Continue [no]", b"\n")], False)
                assert Path(config).read_bytes() == before_reset
                setup_terminal(["config", "reset"], [
                    (b"Continue [no]", b"yes\n"),
                    (b"Board name", b"Cancelled board\n"),
                    (b"Listen address [", b"\x04"),
                ], False)
                assert Path(config).read_bytes() == before_reset
                wizard_path = Path(directory) / "wizard.toml"
                setup_terminal(["init"], [
                    (b"Board name", b"Wizard board\n"),
                    (b"Listen address [", b"\n"),
                    (b"Port [", b"\n"),
                    (b"Camera count", b"\n"),
                ], True, config_path=str(wizard_path))
                assert 'name = "Wizard board"' in wizard_path.read_text()
                wizard_path.write_text("invalid TOML [")
                repaired = subprocess.run([binary, "--config", str(wizard_path), "--json", "config", "reset", "--yes", "--defaults"], check=True, capture_output=True, text=True)
                assert json.loads(repaired.stdout)["server"]["port"] == 5180

                daemon = start(True)
                initial = cli("status")
                assert initial["state"] == "stopped"
                duplicate = subprocess.run(base + ["run"], capture_output=True, text=True, timeout=5)
                assert duplicate.returncode != 0 and "in use" in duplicate.stderr
                saved = Path(config).read_bytes()
                assert "in use" in cli("config", "reset", "--yes", "--defaults", ok=False)["error"]
                assert Path(config).read_bytes() == saved
                subprocess.run([binary, "--config", str(Path(directory) / "second.toml"), "config", "init", "--defaults"], check=True, capture_output=True)
                conflict = subprocess.run([binary, "--config", str(Path(directory) / "second.toml"), "--port", str(port), "run"], capture_output=True, text=True, timeout=5)
                assert conflict.returncode != 0 and "cannot listen" in conflict.stderr
                assert "not_implemented" in cli("calibrate", "--all", ok=False)["error"]
                assert cli("config", "validate")["valid"]
                assert cli("cameras", "list")["capture_available"] is False

                events = subprocess.Popen(base + ["events", "--json", "--count", "4"], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    assert select.select([events.stdout], [], [], 5)[0], "missing initial event"
                    first = json.loads(events.stdout.readline())
                    assert first["kind"] == "snapshot"
                    cli("start")
                    cli("demo", "throw", "T20")
                    cli("demo", "takeout")
                    output, errors = events.communicate(timeout=5)
                    assert events.returncode == 0, errors
                    assert [json.loads(line)["kind"] for line in output.splitlines()] == ["started", "throw_accepted", "takeout_completed"]
                finally:
                    if events.poll() is None:
                        events.kill()
                        events.wait()

                cli("demo", "throw", "D20")
                reset = cli("reset")
                assert reset["physical_darts"] == 1 and reset["throws"] == []
                cli("demo", "takeout")
                renamed = cli("config", "set", "board.name", "Smoke board")
                assert renamed["config"]["board"]["id"] == initial["config"]["board"]["id"]
                assert cli("stop")["state"] == "stopped"
                assert cli("wake")["state"] == "running"

                # The actual TUI must edit through the daemon and restore terminal state.
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
                before = termios.tcgetattr(slave)
                ui = subprocess.Popen(base + ["remote"], stdin=slave, stdout=slave, stderr=slave, env={**os.environ, "TERM": "xterm-256color"})
                rendered = bytearray()
                try:
                    def connected():
                        if select.select([master], [], [], 0.1)[0]:
                            rendered.extend(os.read(master, 65536))
                        return b"CONNECTED" in rendered
                    eventually(connected)
                    os.write(master, b"3\r\x15Smoke room\r")
                    eventually(lambda: cli("status")["config"]["board"]["name"] == "Smoke room")

                    # SGR mouse input, as emitted by xterm.js and native terminals.
                    def click(column, row):
                        os.write(master, f"\x1b[<0;{column};{row}M\x1b[<0;{column};{row}m".encode())

                    assert b"\x1b[?1006h" in rendered, "mouse reporting was not enabled"
                    click(14, 28)  # Stop in the footer.
                    eventually(lambda: cli("status")["state"] == "stopped")
                    click(4, 28)  # Start in the footer.
                    eventually(lambda: cli("status")["state"] == "running")
                    click(3, 5)   # Overview tab.
                    click(25, 5)  # Configuration tab.
                    click(5, 10)  # Board name row.
                    os.write(master, b"\x15Mouse room")
                    click(20, 17)  # Save in the editor.
                    eventually(lambda: cli("status")["config"]["board"]["name"] == "Mouse room")
                    click(5, 11)  # Speed row.
                    click(28, 17)  # Next choice.
                    click(38, 17)  # Save choice.
                    eventually(lambda: cli("status")["config"]["detection"]["speed"] == "high")
                    click(44, 28)  # Quit in the footer.
                    assert ui.wait(timeout=5) == 0
                    assert termios.tcgetattr(slave) == before, "terminal state not restored"
                    while select.select([master], [], [], 0.1)[0]:
                        rendered.extend(os.read(master, 65536))
                    assert b"\x1b[?1006l" in rendered, "mouse reporting was not disabled"
                finally:
                    if ui.poll() is None:
                        ui.kill()
                        ui.wait()
                    os.close(master)
                    os.close(slave)

                # Shutdown must complete even while an SSE consumer remains connected.
                stream = urllib.request.urlopen(f"http://127.0.0.1:{port}/api/v1/events", timeout=5)
                stream.readline()
                daemon.terminate()
                assert daemon.wait(timeout=5) == 0
                stream.close()
                daemon = start(False)
                restarted = cli("status")
                assert restarted["config"]["board"]["name"] == "Mouse room"
                assert restarted["config"]["board"]["id"] == initial["config"]["board"]["id"]
                assert restarted["session_id"] != initial["session_id"]
                assert restarted["physical_darts"] is None
                assert "disabled" in cli("demo", "throw", "T20", ok=False)["error"]
                print("PASS: init/reset and setup cancellation, CLI, TUI keyboard/mouse editing and restoration, native events, identity persistence, demo isolation, port/ownership conflicts, and streaming shutdown")
            except BaseException:
                log.flush()
                log.seek(0)
                print(log.read())
                raise
            finally:
                if daemon is not None and daemon.poll() is None:
                    daemon.terminate()
                    daemon.wait(timeout=5)


if __name__ == "__main__":
    main()
