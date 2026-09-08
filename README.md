# xmip-core-logic-soap

The `soap` logic technology, a technology of
[xmip-core-logic](https://github.com/IlleNilsson/xmip-core-logic): an Envelope on HTTP: the Body's first child names the operation and is the XML arguments; a result or a Fault goes back inside an Envelope, in the version that arrived.

ADR-0043: a Logic technology turns a Stream that arrived on a transport into a
named operation with typed arguments, and an operation's result back into a
Stream, using a contract to type both. Both directions live here: a Receive
Location reads invocations and writes replies, a Send Location writes requests
and reads outcomes.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
