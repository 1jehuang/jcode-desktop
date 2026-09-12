//! Submission regressions use the real session worker and SDK over socket pairs.

use super::*;
use jcode_sdk::api::ErrorCode;

fn busy_error() -> ApiEvent {
    ApiEvent::Error {
        code: ErrorCode::Internal,
        message: "Already processing a message".into(),
    }
}

#[test]
fn busy_recovery_retains_images_and_never_replays_twice() {
    let images = vec![("image/png".into(), "original image bytes".into())];
    let mut unaccepted = VecDeque::from([SessionCommand::Send {
        content: "look at this".into(),
        images: images.clone(),
    }]);
    let mut pending = VecDeque::from([SessionCommand::Cancel]);
    assert!(recover_async_busy(
        &busy_error(),
        &mut unaccepted,
        &mut pending
    ));
    assert!(!recover_async_busy(
        &busy_error(),
        &mut unaccepted,
        &mut pending
    ));
    assert!(
        matches!(pending.pop_front(), Some(SessionCommand::Send { content, images: actual })
        if content == "look at this" && actual == images)
    );
    assert!(matches!(pending.pop_front(), Some(SessionCommand::Cancel)));
}

#[test]
fn ordinary_errors_do_not_trigger_busy_recovery() {
    let mut unaccepted = VecDeque::from([SessionCommand::Send {
        content: "prompt".into(),
        images: vec![],
    }]);
    let mut pending = VecDeque::new();
    let event = ApiEvent::Error {
        code: ErrorCode::Internal,
        message: "provider rejected image".into(),
    };
    assert!(!recover_async_busy(&event, &mut unaccepted, &mut pending));
    assert_eq!(unaccepted.len(), 1);
    assert!(pending.is_empty());
}

#[cfg(unix)]
mod socket_tests {
    use super::*;
    include!("harness_recovery_tests.rs");
    use jcode_sdk::api::{ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Instant;

    struct Transport(UnixStream);
    impl jcode_sdk::Transport for Transport {
        fn split(
            self: Box<Self>,
        ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
            let writer = self.0.try_clone().unwrap();
            Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
        }
    }

    struct Worker {
        commands: Sender<SessionCommand>,
        updates: async_channel::Receiver<Update>,
        requests: Receiver<ApiRequest>,
        writer: Arc<Mutex<UnixStream>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Worker {
        fn new(mut on_send: impl FnMut(&ApiRequest) -> Vec<ApiEvent> + Send + 'static) -> Self {
            let (client_socket, server_socket) = UnixStream::pair().unwrap();
            let mut reader = BufReader::new(server_socket.try_clone().unwrap());
            let writer = Arc::new(Mutex::new(server_socket));
            let server_writer = writer.clone();
            let (request_tx, requests) = channel();
            std::thread::spawn(move || {
                while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
                    let event = match &frame.request {
                        ApiRequest::Hello { .. } => ApiEvent::HelloOk {
                            version: jcode_sdk::api::API_VERSION_MAJOR,
                            server: "submission-regression".into(),
                            capabilities: vec![],
                        },
                        ApiRequest::GetHistory { .. } => ApiEvent::History {
                            session_id: "s1".into(),
                            messages: vec![],
                            images: vec![],
                        },
                        ApiRequest::GetRuntimeInfo { .. } => ApiEvent::RuntimeInfo {
                            session_id: "s1".into(),
                            provider: None,
                            model: None,
                            routes: vec![],
                            reasoning_effort: None,
                        },
                        ApiRequest::SendMessage { .. } => {
                            let events = on_send(&frame.request);
                            request_tx.send(frame.request).unwrap();
                            for event in events {
                                write_frame(
                                    &mut *server_writer.lock().unwrap(),
                                    &ServerFrame::event(event),
                                )
                                .unwrap();
                            }
                            continue;
                        }
                        ApiRequest::SoftInterrupt { .. } => {
                            request_tx.send(frame.request.clone()).unwrap();
                            ApiEvent::Ok
                        }
                        _ => ApiEvent::Ok,
                    };
                    if write_frame(
                        &mut *server_writer.lock().unwrap(),
                        &ServerFrame::reply(frame.id, event),
                    )
                    .is_err()
                    {
                        break;
                    }
                }
            });
            let client = JcodeClient::connect_with(
                Box::new(Transport(client_socket)),
                ConnectOptions {
                    ensure_runtime: false,
                    request_timeout: Some(Duration::from_secs(2)),
                    ..Default::default()
                },
            )
            .unwrap();
            let (commands, command_rx) = channel();
            let (update_tx, updates) = async_channel::unbounded();
            let thread = std::thread::spawn(move || {
                session_worker(
                    "s1".into(),
                    command_rx,
                    UpdateSender(update_tx),
                    Some(client),
                )
            });
            let worker = Self {
                commands,
                updates,
                requests,
                writer,
                thread: Some(thread),
            };
            worker.wait_for(|update| {
                matches!(
                    update,
                    Update::Event {
                        event: ApiEvent::RuntimeInfo { .. },
                        ..
                    }
                )
            });
            worker
        }

        fn send(&self, content: &str, images: Vec<(String, String)>) {
            self.commands
                .send(SessionCommand::Send {
                    content: content.into(),
                    images,
                })
                .unwrap();
        }

        fn request(&self) -> ApiRequest {
            self.requests
                .recv_timeout(Duration::from_secs(3))
                .expect("worker must submit promptly")
        }

        fn event(&self, event: ApiEvent) {
            write_frame(
                &mut *self.writer.lock().unwrap(),
                &ServerFrame::event(event),
            )
            .unwrap();
        }

        fn wait_for(&self, matches: impl Fn(&Update) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                assert!(
                    Instant::now() < deadline,
                    "worker did not emit expected update"
                );
                if let Ok(update) = self.updates.try_recv() {
                    assert!(
                        !matches!(
                            update,
                            Update::SendFailed { .. }
                                | Update::Event {
                                    event: ApiEvent::Error { .. },
                                    ..
                                }
                        ),
                        "unexpected error: {update:?}"
                    );
                    if matches(&update) {
                        return;
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.commands.send(SessionCommand::Stop);
            self.thread.take().unwrap().join().unwrap();
            let _ = self
                .writer
                .lock()
                .unwrap()
                .shutdown(std::net::Shutdown::Both);
        }
    }

    fn images() -> Vec<(String, String)> {
        vec![("image/png".into(), "same-image-payload".into())]
    }

    fn assert_steering(
        request: ApiRequest,
        expected: &str,
        expected_images: Vec<(String, String)>,
    ) {
        assert!(
            matches!(request, ApiRequest::SoftInterrupt { session_id, content, images, urgent }
            if session_id == "s1" && content == expected && images == expected_images && urgent)
        );
    }

    #[test]
    fn repeated_image_followups_use_steering_without_duplicate_normal_sends() {
        let worker = Worker::new(|_| {
            vec![ApiEvent::MessageAccepted {
                session_id: "s1".into(),
            }]
        });
        worker.send("first image", images());
        worker.send("same image again", images());
        worker.send("text followup", vec![]);
        assert!(
            matches!(worker.request(), ApiRequest::SendMessage { content, images: actual, .. }
            if content == "first image" && actual == images())
        );
        assert_steering(worker.request(), "same image again", images());
        assert_steering(worker.request(), "text followup", vec![]);
        worker.wait_for(|update| {
            matches!(
                update,
                Update::Event {
                    event: ApiEvent::MessageAccepted { .. },
                    ..
                }
            )
        });
        assert!(worker.requests.try_recv().is_err());
    }

    #[test]
    fn async_busy_rejection_retries_image_and_text_through_steering() {
        for payload in [images(), vec![]] {
            let worker = Worker::new(|_| vec![busy_error()]);
            worker.send("race followup", payload.clone());
            assert!(matches!(worker.request(), ApiRequest::SendMessage { .. }));
            assert_steering(worker.request(), "race followup", payload);
            // A fence confirms the event loop handled the rejection without
            // forwarding it to the transcript or issuing another normal send.
            worker.event(ApiEvent::TextDelta {
                session_id: "s1".into(),
                text: "continued".into(),
            });
            worker.wait_for(|update| {
                matches!(
                    update,
                    Update::Event {
                        event: ApiEvent::TextDelta { .. },
                        ..
                    }
                )
            });
            assert!(worker.requests.try_recv().is_err());
        }
    }

    #[test]
    fn acknowledged_steering_is_not_replayed_after_a_later_busy_rejection() {
        let mut sends = 0;
        let worker = Worker::new(move |_| {
            sends += 1;
            if sends == 1 {
                vec![ApiEvent::MessageAccepted {
                    session_id: "s1".into(),
                }]
            } else {
                vec![busy_error()]
            }
        });
        worker.send("initial", vec![]);
        assert!(matches!(worker.request(), ApiRequest::SendMessage { .. }));
        worker.wait_for(|update| {
            matches!(
                update,
                Update::Event {
                    event: ApiEvent::MessageAccepted { .. },
                    ..
                }
            )
        });
        worker.send("already delivered", images());
        assert_steering(worker.request(), "already delivered", images());
        worker.event(ApiEvent::TurnDone {
            session_id: "s1".into(),
        });
        worker.wait_for(|update| {
            matches!(
                update,
                Update::Event {
                    event: ApiEvent::TurnDone { .. },
                    ..
                }
            )
        });
        worker.send("new followup", images());
        assert!(
            matches!(worker.request(), ApiRequest::SendMessage { content, .. } if content == "new followup")
        );
        assert_steering(worker.request(), "new followup", images());
        worker.event(ApiEvent::TextDelta {
            session_id: "s1".into(),
            text: "continued".into(),
        });
        worker.wait_for(|update| {
            matches!(
                update,
                Update::Event {
                    event: ApiEvent::TextDelta { .. },
                    ..
                }
            )
        });
        assert!(worker.requests.try_recv().is_err());
    }
}
