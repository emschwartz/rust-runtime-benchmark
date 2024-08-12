use crate::num_cpus;
use crate::support::{Body, TokioIo};
use hyper::service::service_fn;
use hyper::{Error, Response};
use moro::async_scope;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{channel, Sender};

#[cfg(feature = "tokio-single-thread")]
pub fn single_thread_server() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async move {
        let service = service_fn(|_| async {
            Ok::<_, Error>(Response::new(Body::from("Hello world!".to_string())))
        });

        let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
        let listener = TcpListener::bind(addr).await.expect("Bind error");

        async_scope!(|scope| {
            loop {
                let (stream, _) = listener.accept().await.expect("Accept error");
                let io = TokioIo::new(stream);

                scope.spawn(async {
                    if let Err(err) = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await
                    {
                        println!("Error serving connection: {:?}", err);
                    }
                });
            }
        })
        .await;
    });
}

#[cfg(feature = "tokio-work-stealing")]
pub fn work_stealing_server() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_cpus())
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
        let listener = TcpListener::bind(addr).await.expect("Bind error");

        let service = service_fn(move |_| async {
            Ok::<_, Error>(Response::new("Hello world!".to_string()))
        });

        loop {
            let (stream, _) = listener.accept().await.expect("Accept error");

            let io = TokioIo::new(stream);
            let service = service.clone();

            tokio::task::spawn(async move {
                if let Err(err) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                {
                    println!("Error serving connection: {:?}", err);
                }
            });
        }
    });
}

#[cfg(feature = "tokio-active-connection-count")]
pub fn active_connection_count_server() {
    let num_workders: usize = num_cpus() - 1;
    let mut handles: Vec<(Sender<TcpStream>, Arc<AtomicU32>)> = Vec::new();

    for _ in 0..num_workders {
        let (tx, mut rx) = channel(1);
        let connection_count = Arc::new(AtomicU32::new(0));
        handles.push((tx, connection_count.clone()));
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");

            rt.block_on(async move {
                let service = service_fn(|_| async {
                    Ok::<_, Error>(Response::new(Body::from("Hello world!".to_string())))
                });

                async_scope!(|scope| {
                    loop {
                        let io = if let Some(stream) = rx.recv().await {
                            TokioIo::new(stream)
                        } else {
                            break;
                        };

                        connection_count.fetch_add(1, Ordering::Relaxed);

                        scope.spawn(async {
                            if let Err(err) = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, service)
                                .await
                            {
                                println!("Error serving connection: {:?}", err);
                            }

                            connection_count.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                })
                .await;
            })
        });
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
        let listener = TcpListener::bind(addr).await.expect("Bind error");

        loop {
            let (stream, _) = listener.accept().await.expect("Accept error");
            handles.sort_by_key(|(_, count)| count.load(Ordering::Relaxed));
            let conn = handles.first().unwrap().0.clone();
            tokio::spawn(async move { conn.send(stream).await });
        }
    });
}

#[cfg(feature = "tokio-round-robin")]
pub fn round_robin_server() {
    let num_workers: usize = num_cpus() - 1;
    let mut handles: Vec<tokio::sync::mpsc::Sender<TcpStream>> = Vec::new();

    for _ in 0..num_workers {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        handles.push(tx);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime");

            rt.block_on(async move {
                let service = service_fn(|_| async {
                    Ok::<_, Error>(Response::new(Body::from("Hello world!".to_string())))
                });

                async_scope!(|scope| {
                    loop {
                        let io = if let Some(stream) = rx.recv().await {
                            TokioIo::new(stream)
                        } else {
                            break;
                        };

                        scope.spawn(async {
                            if let Err(err) = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, service)
                                .await
                            {
                                println!("Error serving connection: {:?}", err);
                            }
                        });
                    }
                })
                .await;
            })
        });
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
        let listener = TcpListener::bind(addr).await.expect("Bind error");

        let mut worker_index = 0;
        loop {
            let (stream, _) = listener.accept().await.expect("Accept error");
            let conn = handles[worker_index].clone();
            tokio::spawn(async move { conn.send(stream).await });

            worker_index = (worker_index + 1) % num_workers;
        }
    });
}