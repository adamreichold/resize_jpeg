export RUSTFLAGS="-Ctarget-cpu=skylake"
export CFLAGS="-march=skylake"

cargo build --release
