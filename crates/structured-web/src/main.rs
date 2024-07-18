/// This example shows how to use hyper with a single-threaded runtime.
/// This example exists also to test if the code compiles when `Body` is not `Send`.
///
/// This Example includes HTTP/1 and HTTP/2 server and client.
///
/// In HTTP/1 it is possible to use a `!Send` `Body`type.
/// In HTTP/2 it is possible to use a `!Send` `Body` and `IO` type.
///
/// The `Body` and `IOTypeNotSend` structs in this example are `!Send`
///
/// For HTTP/2 this only works if the `Executor` trait is implemented without the `Send` bound.
use http_body_util::BodyExt;
use hyper::server::conn::http2;
use std::cell::Cell;
use std::net::SocketAddr;
use std::rc::Rc;
use tokio::io::{self, AsyncWriteExt};
use tokio::net::TcpListener;

use hyper::body::{Body as HttpBody, Bytes, Frame};
use hyper::service::service_fn;
use hyper::Request;
use hyper::{Error, Response};
use moro::async_scope;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::thread;
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

    let server_http2 = thread::spawn(move || {
        // Configure a runtime for the server that runs everything on the current thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");

        rt.block_on(http2_server()).unwrap();
    });

    let client_http2 = thread::spawn(move || {
        // Configure a runtime for the client that runs everything on the current thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");

        rt.block_on(http2_client(
            "http://localhost:3000".parse::<hyper::Uri>().unwrap(),
        ))
        .unwrap();
    });

    let server_http1 = thread::spawn(move || {
        // Configure a runtime for the server that runs everything on the current thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");

        rt.block_on(http1_server()).unwrap();
    });

    let client_http1 = thread::spawn(move || {
        // Configure a runtime for the client that runs everything on the current thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");

        rt.block_on(http1_client(
            "http://localhost:3001".parse::<hyper::Uri>().unwrap(),
        ))
        .unwrap();
    });

    server_http2.join().unwrap();
    client_http2.join().unwrap();

    server_http1.join().unwrap();
    client_http1.join().unwrap();
}

async fn http1_server() -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 3001));

    let listener = TcpListener::bind(addr).await?;

    let counter = Cell::new(0);

    let service = service_fn(|_| async {
        let prev = counter.get();
        counter.set(prev + 1);
        let value = counter.get();
        Ok::<_, Error>(Response::new(Body::from(format!("Request #{}", value))))
    });

    async_scope!(|scope| {
        loop {
            let (stream, _) = listener.accept().await?;

            let io = IOTypeNotSend::new(TokioIo::new(stream));

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
    .await
}

async fn http1_client(url: hyper::Uri) -> Result<(), Box<dyn std::error::Error>> {
    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(80);
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(addr).await?;

    let io = IOTypeNotSend::new(TokioIo::new(stream));

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

    async_scope!(|scope| {
        scope.spawn(async move {
            if let Err(err) = conn.await {
                let mut stdout = io::stdout();
                stdout
                    .write_all(format!("Connection failed: {:?}", err).as_bytes())
                    .await
                    .unwrap();
                stdout.flush().await.unwrap();
            }
        });

        let authority = url.authority().unwrap().clone();

        // Make 4 requests
        for _ in 0..4 {
            let req = Request::builder()
                .uri(url.clone())
                .header(hyper::header::HOST, authority.as_str())
                .body(Body::from("test".to_string()))?;

            let mut res = sender.send_request(req).await?;

            let mut stdout = io::stdout();
            stdout
                .write_all(format!("Response: {}\n", res.status()).as_bytes())
                .await
                .unwrap();
            stdout
                .write_all(format!("Headers: {:#?}\n", res.headers()).as_bytes())
                .await
                .unwrap();
            stdout.flush().await.unwrap();

            // Print the response body
            while let Some(next) = res.frame().await {
                let frame = next?;
                if let Some(chunk) = frame.data_ref() {
                    stdout.write_all(&chunk).await.unwrap();
                }
            }
            stdout.write_all(b"\n-----------------\n").await.unwrap();
            stdout.flush().await.unwrap();
        }
        Ok(())
    })
    .await
}

async fn http2_server() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();

    let addr: SocketAddr = ([127, 0, 0, 1], 3000).into();
    // Using a !Send request counter is fine on 1 thread...
    let counter = Cell::new(0);

    let listener = TcpListener::bind(addr).await?;

    stdout
        .write_all(format!("Listening on http://{}", addr).as_bytes())
        .await
        .unwrap();
    stdout.flush().await.unwrap();

    let service = service_fn(|_| {
        let prev = counter.get();
        counter.set(prev + 1);
        let value = counter.get();
        async move { Ok::<_, Error>(Response::new(Body::from(format!("Request #{}", value)))) }
    });

    async_scope!(|scope| {
        loop {
            let (stream, _) = listener.accept().await?;
            let io = IOTypeNotSend::new(TokioIo::new(stream));

            scope.spawn(async {
                if let Err(err) = http2::Builder::new(LocalExec::new(scope))
                    .serve_connection(io, service)
                    .await
                {
                    let mut stdout = io::stdout();
                    stdout
                        .write_all(format!("Error serving connection: {:?}", err).as_bytes())
                        .await
                        .unwrap();
                    stdout.flush().await.unwrap();
                }
            });
        }
    })
    .await
}

async fn http2_client(url: hyper::Uri) -> Result<(), Box<dyn std::error::Error>> {
    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(80);
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(addr).await?;

    let stream = IOTypeNotSend::new(TokioIo::new(stream));

    async_scope!(|scope| {
        let (mut sender, conn) =
            hyper::client::conn::http2::handshake(LocalExec::new(scope), stream).await?;

        scope.spawn(async {
            if let Err(err) = conn.await {
                let mut stdout = io::stdout();
                stdout
                    .write_all(format!("Connection failed: {:?}", err).as_bytes())
                    .await
                    .unwrap();
                stdout.flush().await.unwrap();
            }
        });

        let authority = url.authority().unwrap().clone();

        // Make 4 requests
        for _ in 0..4 {
            let req = Request::builder()
                .uri(url.clone())
                .header(hyper::header::HOST, authority.as_str())
                .body(Body::from("test".to_string()))?;

            let mut res = sender.send_request(req).await?;

            let mut stdout = io::stdout();
            stdout
                .write_all(format!("Response: {}\n", res.status()).as_bytes())
                .await
                .unwrap();
            stdout
                .write_all(format!("Headers: {:#?}\n", res.headers()).as_bytes())
                .await
                .unwrap();
            stdout.flush().await.unwrap();

            // Print the response body
            while let Some(next) = res.frame().await {
                let frame = next?;
                if let Some(chunk) = frame.data_ref() {
                    stdout.write_all(&chunk).await.unwrap();
                }
            }
            stdout.write_all(b"\n-----------------\n").await.unwrap();
            stdout.flush().await.unwrap();
        }
        Ok(())
    })
    .await
}

// NOTE: This part is only needed for HTTP/2. HTTP/1 doesn't need an executor.
//
// Since the Server needs to spawn some background tasks, we needed
// to configure an Executor that can spawn !Send futures...

struct LocalExec<'scope, 'env, R: 'env> {
    scope: Rc<&'scope moro::Scope<'scope, 'env, R>>,
}

impl<'scope, 'env, R: 'env> LocalExec<'scope, 'env, R> {
    fn new(scope: &'scope moro::Scope<'scope, 'env, R>) -> Self {
        Self {
            scope: Rc::new(scope),
        }
    }
}

// We need a manual implementation of Clone so that it uses Rc::clone
// (because Scope isn't Clone)
impl<'scope, 'env, R: 'env> Clone for LocalExec<'scope, 'env, R> {
    fn clone(&self) -> Self {
        Self {
            scope: Rc::clone(&self.scope),
        }
    }
}

impl<'scope, 'env, F, R: 'env> hyper::rt::Executor<F> for LocalExec<'scope, 'env, R>
where
    F: std::future::Future + 'scope,
{
    fn execute(&self, fut: F) {
        self.scope.spawn(fut);
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
