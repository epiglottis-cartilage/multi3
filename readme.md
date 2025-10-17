# multi 3

With a speed limitation 3 Mb/s for each ip addr in my school,
I develop this proxy server that can assign tasks to different addresses to speed up Multi-threaded network tasks.

## Usage

First you should have multi useable ip addr.
You can config your PC to use static ip addr and assign lots of adders,
or you can just buy many adaptors.
Then edit `multi3.toml` and list then at the `pool`.

Use `cargo run --release` to compile and run the program.
Or you can start executable at the same directory with `multi3.toml`.

Don't forget manually setup system proxy.

## Note

Now it can handle both HTTP(S) and SOCKS5 proxy.
