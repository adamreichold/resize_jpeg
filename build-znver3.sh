export RUSTFLAGS="-Ctarget-cpu=znver3"
export CFLAGS="-march=znver3"

cargo build --release
