# Kanal
**Fastest sync and async channel that Rust deserves!**

## What is Kanal
Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.

**Performance is the main goal of Kanal.**

### Benchmark Results
Results are normalized based on kanal sync results, so 60x means the test for that framework takes 60 times slower than kanal.

Machine: `AMD Ryzen Threadripper 2950X 16-Core Processor`<br />
Rust: `rustc rustc 1.64.0`<br />
Go: `go version go1.19.2 linux/amd64`<br />
OS (`uname -a`): `Linux 5.13.0-35-generic #40~20.04.1-Ubuntu SMP Mon Mar 7 09:18:32 UTC 2022 x86_64`<br />
Date: Oct 16, 2022

[Benchmark codes](https://github.com/fereidani/rust-channel-benchmarks)

![Benchmarks](https://i.imgur.com/gHfk5fy.png)
