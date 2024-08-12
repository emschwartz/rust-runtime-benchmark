# Installing on a fresh machine

```sh
sudo apt update
sudo apt install build-essential git liburing-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

cargo run --release

wrk -t12 -c400 -d30s --latency http://127
