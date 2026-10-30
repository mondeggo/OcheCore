//! Versioned native HTTP API. No Autodarts routes are emulated here.
mod cameras;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::Stream;
use oche_domain::*;
use oche_engine::{Engine, error, live::Pipeline};
use std::{
    convert::Infallible,
    future::{Future, IntoFuture},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, broadcast, watch},
};

#[derive(Clone)]
pub struct Service {
    engine: Arc<Mutex<Engine>>,
    requests: Arc<Semaphore>,
    subscribers: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
    camera_probe: Arc<Semaphore>,
    camera_cache: Arc<Mutex<Option<(Instant, CameraInventory)>>>,
    pipeline: Option<Arc<Pipeline>>,
}

impl Service {
    pub fn new(engine: Engine) -> Self {
        Self::shared(Arc::new(Mutex::new(engine)), None)
    }

    /// A service over an engine that a live camera pipeline also observes.
    pub fn shared(engine: Arc<Mutex<Engine>>, pipeline: Option<Arc<Pipeline>>) -> Self {
        Self {
            pipeline,
            engine,
            requests: Arc::new(Semaphore::new(32)),
            subscribers: Arc::new(Semaphore::new(16)),
            shutdown: watch::channel(false).0,
            camera_probe: Arc::new(Semaphore::new(1)),
            camera_cache: Arc::new(Mutex::new(None)),
        }
    }

    async fn access<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Engine) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, Failure> {
        let permit = self
            .requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| Failure(error("busy", "control service is busy; retry shortly")))?;
        let engine = self.engine.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut engine = engine
                .lock()
                .map_err(|_| Failure(error("internal_error", "command authority unavailable")))?;
            action(&mut engine).map_err(Failure)
        })
        .await
        .map_err(|_| Failure(error("internal_error", "command task failed")))?
    }
}

pub struct Failure(ApiError);

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "invalid_request" => StatusCode::BAD_REQUEST,
            "conflict" => StatusCode::CONFLICT,
            "calibration_failed" => StatusCode::UNPROCESSABLE_ENTITY,
            "not_implemented" => StatusCode::NOT_IMPLEMENTED,
            "busy" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}

fn payload<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, Failure> {
    input
        .map(|Json(value)| value)
        .map_err(|e| Failure(error("invalid_request", e.body_text())))
}

pub fn router(service: Service) -> Router {
    Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/cameras/available", get(available_cameras))
        .route("/api/v1/cameras/{slot}/snapshot", get(camera_snapshot))
        .route("/api/v1/config", get(config).put(configure))
        .route("/api/v1/command", post(command))
        .route("/api/v1/events", get(events))
        .route("/api/v1/events/recent", get(history))
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(error("not_found", "unknown native API route")),
            )
        })
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(service)
}

async fn status(State(service): State<Service>) -> Result<Json<Snapshot>, Failure> {
    service.access(|e| Ok(e.snapshot())).await.map(Json)
}

async fn available_cameras(
    State(service): State<Service>,
) -> Result<Json<CameraInventory>, Failure> {
    if let Some((time, inventory)) = service.camera_cache.lock().unwrap().as_ref()
        && time.elapsed() < Duration::from_secs(5)
    {
        return Ok(Json(inventory.clone()));
    }
    let permit = service
        .camera_probe
        .clone()
        .try_acquire_owned()
        .map_err(|_| Failure(error("busy", "camera discovery is already running")))?;
    let cache = service.camera_cache.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let inventory = cameras::discover();
        *cache.lock().unwrap() = Some((Instant::now(), inventory.clone()));
        Json(inventory)
    })
    .await
    .map_err(|_| Failure(error("internal_error", "camera discovery failed")))
}

async fn config(State(service): State<Service>) -> Result<Json<Config>, Failure> {
    service.access(|e| Ok(e.snapshot().config)).await.map(Json)
}

async fn command(
    State(service): State<Service>,
    body: Result<Json<Command>, JsonRejection>,
) -> Result<Json<Snapshot>, Failure> {
    let command = payload(body)?;
    if let (Command::Calibrate { camera }, Some(pipeline)) = (&command, service.pipeline.clone()) {
        // Measuring takes seconds; run it without holding the command lock,
        // then install the validated result through it.
        let camera = *camera;
        let record = tokio::task::spawn_blocking(move || pipeline.calibrate(camera))
            .await
            .map_err(|_| Failure(error("internal_error", "calibration task failed")))?
            .map_err(Failure)?;
        return service
            .access(move |e| e.install_calibration(record))
            .await
            .map(Json);
    }
    if let (Command::OrientBegin | Command::OrientFinish, Some(pipeline)) =
        (&command, service.pipeline.clone())
    {
        let finish = matches!(command, Command::OrientFinish);
        let hints = tokio::task::spawn_blocking(move || {
            if finish {
                pipeline.orient_finish()
            } else {
                pipeline.orient_begin().map(|()| Vec::new())
            }
        })
        .await
        .map_err(|_| Failure(error("internal_error", "orientation task failed")))?
        .map_err(Failure)?;
        return service
            .access(move |e| {
                // The pipeline verified every hint before returning any.
                for (slot, [x, y]) in hints {
                    e.configure(ConfigChange {
                        key: format!("cameras.{slot}.hint"),
                        value: format!("{x},{y}"),
                        expected_revision: None,
                    })?;
                }
                Ok(e.snapshot())
            })
            .await
            .map(Json);
    }
    service.access(move |e| e.command(command)).await.map(Json)
}

async fn camera_snapshot(
    State(service): State<Service>,
    axum::extract::Path(slot): axum::extract::Path<u8>,
) -> Result<Response, Failure> {
    let pipeline = service.pipeline.clone().ok_or_else(|| {
        Failure(error(
            "conflict",
            "cameras are not captured by this service; run without --demo",
        ))
    })?;
    let jpeg = tokio::task::spawn_blocking(move || pipeline.latest_jpeg(slot))
        .await
        .map_err(|_| Failure(error("internal_error", "snapshot task failed")))?
        .ok_or_else(|| {
            Failure(error(
                "conflict",
                format!("camera {slot} is not streaming; start the board and check its device"),
            ))
        })?;
    Ok(([(axum::http::header::CONTENT_TYPE, "image/jpeg")], jpeg).into_response())
}

async fn configure(
    State(service): State<Service>,
    body: Result<Json<ConfigChange>, JsonRejection>,
) -> Result<Json<Snapshot>, Failure> {
    let change = payload(body)?;
    service.access(move |e| e.configure(change)).await.map(Json)
}

async fn history(State(service): State<Service>) -> Result<Json<Vec<NativeEvent>>, Failure> {
    service.access(|e| Ok(e.history())).await.map(Json)
}

fn sse(event: &NativeEvent) -> Event {
    Event::default()
        .event(&event.kind)
        .id(format!("{}:{}", event.session_id, event.sequence))
        .data(serde_json::to_string(event).expect("native events contain only JSON-safe values"))
}

async fn events(
    State(service): State<Service>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, Failure> {
    let permit = service
        .subscribers
        .clone()
        .try_acquire_owned()
        .map_err(|_| Failure(error("busy", "maximum of 16 event subscribers reached")))?;
    let (initial, mut receiver) = service.access(|e| Ok(e.subscribe())).await?;
    let mut shutdown = service.shutdown.subscribe();
    let stream = async_stream::stream! {
        let _permit = permit;
        let mut sequence = initial.sequence;
        yield Ok(sse(&initial));
        loop {
            let received = tokio::select! {
                _ = shutdown.changed() => break,
                event = receiver.recv() => event,
            };
            let event = match received {
                Ok(event) if event.sequence > sequence => event,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    match service.access(|e| Ok(e.snapshot_event("resync"))).await {
                        Ok(event) => event,
                        Err(_) => break,
                    }
                }
            };
            sequence = event.sequence;
            yield Ok(sse(&event));
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

pub async fn serve(
    listener: TcpListener,
    service: Service,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let tick_service = service.clone();
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let _ = tick_service
                .access(|e| {
                    e.tick(Instant::now());
                    Ok(())
                })
                .await;
        }
    });
    let shutdown_sender = service.shutdown.clone();
    let mut shutting_down = service.shutdown.subscribe();
    let server = axum::serve(listener, router(service));
    let server = server
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = shutdown_sender.send(true);
        })
        .into_future();
    tokio::pin!(server);
    // Close streams and allow at most three seconds for slow HTTP clients.
    let result = tokio::select! {
        result = &mut server => result,
        _ = shutting_down.changed() => {
            tokio::time::timeout(Duration::from_secs(3), &mut server).await.unwrap_or(Ok(()))
        }
    };
    ticker.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use oche_engine::storage::Store;

    #[tokio::test]
    async fn native_clients_share_state_and_receive_snapshot_then_ordered_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::acquire(&dir.path().join("board.toml")).unwrap();
        let config = store.initialize().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let service = Service::new(Engine::new(
            config.clone(),
            store,
            Mode::Demo,
            address.to_string(),
        ));
        let task = tokio::spawn(serve(listener, service, std::future::pending()));
        let client = reqwest::Client::new();
        let url = format!("http://{address}/api/v1");
        let response = client.get(format!("{url}/events")).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let mut stream = response.bytes_stream();
        let initial = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&initial).contains("event: snapshot"));
        for command in [
            Command::Start,
            Command::DemoThrow {
                label: "T20".into(),
            },
            Command::DemoTakeout { remaining: 0 },
        ] {
            assert!(
                client
                    .post(format!("{url}/command"))
                    .json(&command)
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
        }
        let mut received = String::new();
        while !received.contains("event: takeout_completed") {
            let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            received.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(
            received.find("event: started").unwrap()
                < received.find("event: throw_accepted").unwrap()
        );
        let snapshot: Snapshot = client
            .get(format!("{url}/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(snapshot.sequence, 3);
        assert_eq!(snapshot.config.board.id, config.board.id);
        let response = client
            .post(format!("{url}/command"))
            .json(&Command::Calibrate { camera: None })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
        assert_eq!(response.json::<ApiError>().await.unwrap().code, "conflict");
        let response = client
            .post(format!("{url}/command"))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        drop(stream);
        let reconnect = client.get(format!("{url}/events")).send().await.unwrap();
        let chunk = reconnect.bytes_stream().next().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(&chunk).contains("\"sequence\":3"));
        task.abort();
    }
}
