# Build and release automation

Workflows follow Oche's GitHub Releases and multi-architecture Docker pattern.
Version tags publish releases; ordinary pushes/PRs run CI without publishing.

The README's app and Docker badges track separate workflows on `main`:
`ci.yml` compiles the app and runs formatting, lint, unit, and CLI/TUI checks;
`docker.yml` builds the image and runs the offline container smoke test.
The release badge shows the latest stable GitHub Release tag, not the version
declared in Cargo.toml. Build status appears after the workflows run on GitHub;
the release version appears after the first stable release is published.

## Setup

1. Push to the chosen GitHub remote and enable Actions. The README badge URLs
   currently target `mondeggo/ochecore`; update those URLs if using another owner
   or repository name. Live badges require a publicly accessible repository.
2. Allow `GITHUB_TOKEN` to write releases/packages. Permissions are declared per
   job; organization policy must allow them.
3. GHCR needs no extra registry credentials. Set package visibility to public if
   anonymous image downloads are desired.
4. Optionally configure both `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` secrets to
   also publish under `<username>/<repository-name>`, matching Oche's convention.

ARM64 compilation uses `ubuntu-24.04-arm`. If unavailable for the repository's
plan or organization, configure an equivalent ARM64 Linux runner.

## Publish

Update the workspace version in Cargo.toml, refresh Cargo.lock with Cargo, run
the checks, and make a Conventional Commit. Tag the matching version and push:

```sh
git tag v0.1.0
git push origin main
git push origin v0.1.0
```

Use the actual version being released. Tags must match the Cargo version; manual
runs must select an existing version tag. Prereleases like `0.2.0-rc.1` create a
GitHub prerelease and do not move `latest`.

The workflow:

1. Checks formatting/Clippy, tests each target, and compiles static Linux
   `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` executables.
2. Runs real CLI/TUI smoke tests on the corresponding CPU architecture.
3. Bundles dependency licenses from the locked target graph, documentation, and
   project license notices in archives with SHA-256 checksums.
4. Builds both images from those same binaries, tests them without Internet,
   then publishes to GHCR and optionally Docker Hub.
5. Publishes GitHub Release archives, individual checksum files, and `SHA256SUMS`.
   Reruns replace assets for the same tag.

Stable image tags are `vX.Y.Z` and `latest`; labels include source commit, version,
and license. The image contains no compiler or source checkout.

## Offline installation

Download the architecture's archive and checksum on a connected machine and
transfer them to the board. Replace version/architecture in these examples:

```sh
sha256sum -c ochecore-vX.Y.Z-linux-arm64.tar.gz.sha256
tar -xzf ochecore-vX.Y.Z-linux-arm64.tar.gz
./ochecore-vX.Y.Z-linux-arm64/oche-core --version
./ochecore-vX.Y.Z-linux-arm64/oche-core run
```

Keep notices with redistributed binaries. Docker images can be transferred with
`docker save` / `docker load`. Runtime needs no Internet connection.

## Local image checks

```sh
docker build -t ochecore:local .
bash scripts/smoke_container.sh ochecore:local
```

The smoke test uses no external network, a read-only root filesystem, no Linux
capabilities, and a temporary volume. It checks persistence across restart and
removes its resources. Release images use `BINARY_SOURCE=prebuilt` and compiled
inputs under `dist/amd64` and `dist/arm64`.
