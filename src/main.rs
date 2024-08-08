use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{channel, Sender};

use glommio::{LocalExecutorPoolBuilder, PoolPlacement};
use hyper::service::service_fn;
use hyper::{Error, Response};
use moro::async_scope;

mod support;
use support::{Body, TokioIo};
mod glommio;

fn main() {
    pretty_env_logger::init();

    #[cfg(feature = "work-stealing")]
    {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime");

        rt.block_on(http_server_multi_thread()).unwrap();
    }

    #[cfg(feature = "round-robin")]
    round_robin_server();

    #[cfg(feature = "glommio")]
    glommio::glommio_server();

    #[cfg(feature = "active-connection-count")]
    active_connection_count_server();
}

fn active_connection_count_server() {
    let num_cpus: usize = std::thread::available_parallelism().unwrap().get();
    let num_workders: usize = num_cpus - 1;
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
            // pollster::block_on(http_server_thread_per_core(rx)).unwrap();
        });
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

        let listener = TcpListener::bind(addr).await.expect("Bind error");

        loop {
            let (stream, _) = listener.accept().await.expect("Accept error");
            handles.sort_by_key(|(_, count)| count.load(Ordering::Relaxed));
            let conn = handles.first().unwrap().0.clone();
            tokio::spawn(async move { conn.send(stream).await });
        }
    });
}

fn round_robin_server() {
    let num_cpus: usize = std::thread::available_parallelism().unwrap().get();
    let num_workers: usize = num_cpus - 1;
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
            // pollster::block_on(http_server_thread_per_core(rx)).unwrap();
        });
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    rt.block_on(async {
        let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

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

async fn http_server_multi_thread() -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let listener = TcpListener::bind(addr).await?;

    let service =
        service_fn(move |_| async { Ok::<_, Error>(Response::new("Hello world!".to_string())) });

    loop {
        let (stream, _) = listener.accept().await?;

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
}
