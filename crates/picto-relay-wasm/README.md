# picto-relay-wasm

[picto-relay-link](../picto-relay-link/README.md) and
[picto-relay-session](../picto-relay-session/README.md) as a WebAssembly
module, for the web client's demo page, which drives the ESP32 itself over Web
Serial and has no server to do this for it.

```sh
npm --prefix web run wasm    # build it into web/src/serial/picto-relay.wasm
```

`just demo`, `just demo-build` and the web client's tests run that first. It
needs the `wasm32-unknown-unknown` target (`rustup target add
wasm32-unknown-unknown`) and nothing else: there are no generated bindings.

The page passes bytes in one at a time, calls an operation, and takes the
answer out one byte at a time; structured answers are JSON. That is slow in
principle and irrelevant at these sizes, and it means the crate has no unsafe
code beyond the attribute that exports each function by name. The interface is
documented at the top of [`src/lib.rs`](src/lib.rs), and wrapped for the page
by [`web/src/serial/host.ts`](../../web/src/serial/host.ts).
