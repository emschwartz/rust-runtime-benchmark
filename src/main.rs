use std::cell::RefCell;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;

use hyper::body::{Body as HttpBody, Bytes, Frame};
use hyper::service::service_fn;
use hyper::{Error, Response};
use moro::async_scope;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::net::TcpStream;

mod support;
use support::TokioIo;

struct Body {
    // Our Body type is !Send and !Sync:
    _marker: PhantomData<*const ()>,
    data: Option<Bytes>,
}

impl From<String> for Body {
    fn from(a: String) -> Self {
        Body {
            _marker: PhantomData,
            data: Some(a.into()),
        }
    }
}

impl HttpBody for Body {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.get_mut().data.take().map(|d| Ok(Frame::data(d))))
    }
}

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

    #[cfg(feature = "active-connection-count")]
    active_connection_count_server();
}

fn active_connection_count_server() {
    let num_cpus: usize = std::thread::available_parallelism().unwrap().get();
    let num_workders: usize = num_cpus - 1;
    let mut handles: Vec<(tokio::sync::mpsc::Sender<TcpStream>, Arc<AtomicU32>)> = Vec::new();

    for _ in 0..num_workders {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
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

struct IOTypeNotSend {
    _marker: PhantomData<*const ()>,
    stream: TokioIo<TcpStream>,
}

impl IOTypeNotSend {
    fn new(stream: TokioIo<TcpStream>) -> Self {
        Self {
            _marker: PhantomData,
            stream,
        }
    }
}

impl hyper::rt::Write for IOTypeNotSend {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl hyper::rt::Read for IOTypeNotSend {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
