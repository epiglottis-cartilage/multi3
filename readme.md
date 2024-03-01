# multi 3

With a speed limitation 3 Mb/s for each ip addr in my school.

I develop this proxy server that can assign tasks to different addresses to speed up Multi-threaded network tasks.

## Usage

First you should have multi useable ip addr.

You can config your PC to use static ip addr and assign lots of adders.

Or you can just buy many adaptors.

Excuse `ifconfig` (Unix) or `ipconfig` (Win) to list all your ip addr.

Then edit `multi3.toml` and list then at the `pool`.

Use `cargo run --release` to compile and run the program.

Finally don't forget manually setup system proxy server.