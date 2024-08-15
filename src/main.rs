use hyper::{body::Incoming, service::service_fn, Error, Response};

#[cfg(any(feature = "glommio", feature = "glommio-single-thread"))]
mod glommio;
#[cfg(any(
    feature = "tokio-single-thread",
    feature = "tokio-work-stealing",
    feature = "tokio-round-robin",
    feature = "tokio-active-connection-count"
))]
mod support;
#[cfg(any(
    feature = "tokio-single-thread",
    feature = "tokio-work-stealing",
    feature = "tokio-round-robin",
    feature = "tokio-active-connection-count"
))]
mod tokio;

fn main() {
    pretty_env_logger::init();

    #[allow(unused_variables)]
    let service = service_fn(move |_req: hyper::Request<Incoming>| async {
        Ok::<_, Error>(Response::new("Hello world!".to_string()))
    });

    #[cfg(feature = "sleep-service")]
    let service = service_fn(move |_req: hyper::Request<Incoming>| async {
        async {
            sleep(Duration::from_micros(100));
            Ok::<_, Error>(Response::new("Hello world!".to_string()))
        }
        .await
    });

    #[cfg(feature = "random-sleep-service")]
    let service = service_fn(move |_req: hyper::Request<Incoming>| async {
        use rand::Rng;

        async {
            let num_awaits: usize = rand::thread_rng().gen_range(0..=10);
            for _i in 0..num_awaits {
                async {
                    std::thread::sleep(std::time::Duration::from_micros(10));
                }
                .await
            }
            Ok::<_, Error>(Response::new("Hello world!".to_string()))
        }
        .await
    });

    #[cfg(feature = "tokio-work-stealing")]
    tokio::work_stealing_server(service);

    #[cfg(feature = "tokio-single-thread")]
    tokio::single_thread_server(service);

    #[cfg(feature = "glommio")]
    glommio::multi_thread_server(service);

    #[cfg(feature = "glommio-single-thread")]
    glommio::single_thread_server(service);

    #[cfg(feature = "tokio-round-robin")]
    tokio::round_robin_server(service);

    #[cfg(feature = "tokio-active-connection-count")]
    tokio::active_connection_count_server(service);
}

pub fn num_cpus() -> usize {
    let num_cpus: usize = std::env::var("NUM_CPUS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().unwrap().get());
    println!("Using {} cpus", num_cpus);
    num_cpus
}
