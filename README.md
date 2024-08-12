# Benchmarking Tokio and Glommio

## Running the server

You can try out different versions of the server by running:

`cargo run --release --features=glommio`

Where the feature is one of:

- `tokio-work-stealing`
- `tokio-single-thread`
- `glommio`
- `glommio-single-thread`

And you can adjust the number of threads that will be used by the multi-threaded versions using the `NUM_THREADS` environment variable.

Note that you must be running on Linux with a [supported kernel version](https://github.com/DataDog/glommio?tab=readme-ov-file#supported-linux-kernels) in order to run the glommio versions.

## Running the benchmark

You can use the [`wrk`](https://github.com/wg/wrk) tool to run the benchmark.

```
wrk -t4 -c400 -d30s --latency http://localhost:3000
```

## Installing on a fresh Debian machine

```sh
sudo apt update
sudo apt install build-essential git liburing-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```
