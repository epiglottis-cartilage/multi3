# multi 3

With a speed limitation 3 Mb/s for each ip addr in my school,
I develop this proxy server that can assign tasks to different addresses to speed up Multi-threaded network tasks.

## Usage

First you should have multi useable ip addr.

You can config your PC to use static ip addr and assign lots of adders,
or you can just buy many adaptors.

Exec `ifconfig` or `ipconfig`  to list all your ip adders.

Then edit `multi3.toml` and list then at the `pool`.

Use `cargo run --release` to compile and run the program.
Or you can start executable at the same directory with `multi3.toml`.

Don't forget *manually* setup system proxy.

## TLS
It is suspected of intentionally blocking some connections; therefore, a possible solution is to establish multiple connections each time and then select the one that responds fastest.

To use this feature, you need to specify the `tls` field in the configuration file to be greater than 1.

And you need to install the root-certificates in the `cert` folder.