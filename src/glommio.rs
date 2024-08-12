// This code is adapted from Glommio's Hyper example:
// https://github.com/DataDog/glommio/blob/d3f6e7a2ee7fb071ada163edcf90fc3286424c31/examples/hyper_server.rs

mod hyper_compat {
    use futures_lite::{AsyncRead, AsyncWrite, Future};
    use glommio::{
        enclose,
        net::{TcpListener, TcpStream},
        sync::Semaphore,
        GlommioError,
    };
    use hyper::{
        body::{Body as HttpBody, Bytes, Frame, Incoming},
        service::service_fn,
        Error, Request, Response,
    };
    use moro::async_scope;

    use std::{
        io,
        marker::PhantomData,
        net::SocketAddr,
        pin::Pin,
        rc::Rc,
        slice,
        task::{Context, Poll},
    };

    #[derive(Clone)]
    struct HyperExecutor;
    impl<F> hyper::rt::Executor<F> for HyperExecutor
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        fn execute(&self, fut: F) {
            glommio::spawn_local(fut).detach();
        }
    }

    struct HyperStream(pub TcpStream);

    impl hyper::rt::Write for HyperStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.0).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
            Pin::new(&mut self.0).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
            Pin::new(&mut self.0).poll_close(cx)
        }
    }

    impl hyper::rt::Read for HyperStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            mut buf: hyper::rt::ReadBufCursor<'_>,
        ) -> Poll<std::io::Result<()>> {
            unsafe {
                let read_slice = {
                    let buffer = buf.as_mut();
                    buffer.as_mut_ptr().write_bytes(0, buffer.len());
                    slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut u8, buffer.len())
                };
                Pin::new(&mut self.0).poll_read(cx, read_slice).map(|n| {
                    if let Ok(n) = n {
                        buf.advance(n);
                    }
                    Ok(())
                })
            }
        }
    }

    pub struct ResponseBody {
        // Our ResponseBody type is !Send and !Sync
        _marker: PhantomData<*const ()>,
        data: Option<Bytes>,
    }

    impl From<&'static str> for ResponseBody {
        fn from(data: &'static str) -> Self {
            ResponseBody {
                _marker: PhantomData,
                data: Some(Bytes::from(data)),
            }
        }
    }

    impl HttpBody for ResponseBody {
        type Data = Bytes;
        type Error = Error;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(self.get_mut().data.take().map(|d| Ok(Frame::data(d))))
        }
    }

    pub(crate) async fn serve_http1<S, F, R, A>(
        addr: A,
        service: S,
        max_connections: usize,
    ) -> io::Result<()>
    where
        S: Fn(Request<Incoming>) -> F + 'static + Copy,
        F: Future<Output = Result<Response<ResponseBody>, R>>,
        R: std::error::Error + 'static + Send + Sync,
        A: Into<SocketAddr>,
    {
        let listener = TcpListener::bind(addr.into())?;
        let conn_control = Rc::new(Semaphore::new(max_connections as _));
        loop {
            match listener.accept().await {
                Err(x) => {
                    return Err(x.into());
                }
                Ok(stream) => {
                    glommio::spawn_local(enclose! {(conn_control) async move {
                        let addr = stream.local_addr().unwrap();
                        let io = HyperStream(stream);
                        let _permit = conn_control.acquire_permit(1).await;
                        if let Err(err) = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, service_fn(service))
                            .await
                        {
                            if !err.is_incomplete_message() {
                                eprintln!("Stream from {addr:?} failed with error {err:?}");
                            }
                        }
                    }})
                    .detach();
                }
            }
        }
    }

    pub(crate) async fn serve_http2<S, F, R, A>(
        addr: A,
        service: S,
        max_connections: usize,
    ) -> io::Result<()>
    where
        S: Fn(Request<Incoming>) -> F + 'static + Copy,
        F: Future<Output = Result<Response<ResponseBody>, R>> + 'static,
        R: std::error::Error + 'static + Send + Sync,
        A: Into<SocketAddr>,
    {
        let listener = TcpListener::bind(addr.into())?;
        let conn_control = Rc::new(Semaphore::new(max_connections as _));
        loop {
            match listener.accept().await {
                Err(x) => {
                    return Err(x.into());
                }
                Ok(stream) => {
                    let addr = stream.local_addr().unwrap();
                    let io = HyperStream(stream);
                    glommio::spawn_local(enclose! {(conn_control) async move {
                        let _permit = conn_control.acquire_permit(1).await;
                        if let Err(err) = hyper::server::conn::http2::Builder::new(HyperExecutor).serve_connection(io, service_fn(service)).await {
                            if !err.is_incomplete_message() {
                                eprintln!("Stream from {addr:?} failed with error {err:?}");
                            }
                        }
                    }}).detach();
                }
            }
        }
    }
}

use crate::num_cpus;
use glommio::{CpuSet, LocalExecutorBuilder, LocalExecutorPoolBuilder, Placement, PoolPlacement};
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use hyper_compat::ResponseBody;
use std::convert::Infallible;

async fn hyper_demo(req: Request<Incoming>) -> Result<Response<ResponseBody>, Infallible> {
    Ok(Response::new(ResponseBody::from("Hello world!")))
    // match (req.method(), req.uri().path()) {
    //     (&Method::GET, "/hello") => Ok(Response::new(ResponseBody::from("world"))),
    //     _ => Ok(Response::builder()
    //         .status(StatusCode::NOT_FOUND)
    //         .body(ResponseBody::from("notfound"))
    //         .unwrap()),
    // }
}

pub fn multi_thread_server() {
    LocalExecutorPoolBuilder::new(PoolPlacement::MaxSpread(num_cpus(), CpuSet::online().ok()))
        .on_all_shards(|| async {
            let id = glommio::executor().id();
            println!("Starting executor {id}");
            hyper_compat::serve_http1(([0, 0, 0, 0], 3000), hyper_demo, 1024)
                .await
                .unwrap();
        })
        .unwrap()
        .join_all();
}

pub fn single_thread_server() {
    LocalExecutorBuilder::new(Placement::Fixed(0))
        .spawn(|| async {
            hyper_compat::serve_http1(([0, 0, 0, 0], 3000), hyper_demo, 1024)
                .await
                .unwrap();
        })
        .unwrap()
        .join();
}
