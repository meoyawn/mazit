# I want to run youtubei.js from Rust

I want to use [youtubei.js](https://github.com/LuanRT/YouTube.js) from Rust and
keep getting its upstream fixes. YouTube keeps changing. The people maintaining
youtubei.js already do the work of following those changes. I want to update a
package version, rebuild, and get their implementation in my application.

I already had this working with QuickJS. Rust embedded the engine, a JavaScript
bridge called the library, and Rust supplied things like HTTP. The application
could fetch playlists without starting a separate Node process. That was the
baseline, and it mattered: any replacement would have to earn its place against
something that already worked.

But I wanted to see how much of that machinery I could remove. Could I compile
the library ahead of time, link it into Rust, and expose a safe Rust API? Create
a client, await a method, receive an owned value or an error. Still use the npm
implementation. Still get upstream fixes by changing `package.json`.

At first, this looked like a question about compiler output formats.

[scriptc](https://github.com/vercel-labs/scriptc) could emit IR, C, LLVM IR,
assembly, and object files. Which one should Rust consume?

All five modes compiled a small hello-world probe. An object file seemed like
the obvious answer, except that this one defined `main` and expected runtime
symbols at link time. I needed a library with callable exports. Changing the
file extension did not supply that contract.

Scriptc's library mode did. With LLVM emission selected in a library profile,
it produced a static archive that Rust could call:

```sh
scriptc build --lib --profile native-lib.json -o out/libprobe.a
```

Addition returned 42. Strings survived UTF-8, empty input, and embedded NUL
bytes. The archive had the expected C symbols, no `main`, and no QuickJS symbols.
The small native library worked.

Then I gave it youtubei.js.

With scriptc 0.1.4 and youtubei.js 18.0.0, package compilation required the
dynamic engine. Static compilation encountered unsupported imports and exports.
Prebundling with esbuild got past some module structure, but exposed Promise,
type, and parser-class failures. Suppressing type checking reached internal IR
validation errors.

There was also a smaller, decisive reproduction: library mode rejected an
exported synchronous function if anything reachable from it was async. Hiding
an async operation behind `start()` did not help. Sessions, HTTP requests, and
playlist continuations all depend on promises.

Scriptc's dynamic fallback embeds quickjs-ng. I could take that route, but I
already had QuickJS. The experiment had proved the native calling convention;
it had not compiled the package I needed. The
[probe results](experiments/scriptc/README.md) record those as limitations of
the tested compiler version.

I also tried [Perry](https://github.com/PerryTS/perry). Again, a small native
library worked. Rust called its numeric C ABI and got the right answers. Again,
the real package was a different proposition.

An unbundled attempt ran for 25 minutes and 42 seconds before being stopped
without an executable. Bundling did not rescue the experiment: subsequent
attempts also ran for minutes without producing a usable result. That does not
prove Perry could never compile it. It made Perry unsuitable for the update
cycle I wanted. The [Perry notes](experiments/perry/README.md) preserve the
attempts, including the distinction between a working numeric probe and an
unverified full library.

At that point I made the budget explicit: anything slower than a minute was
out. Installing a compiler and its dependencies could be a separate setup cost.
Turning an updated npm package into the implementation Rust would use had to
fit within sixty seconds.

Then Static Hermes compiled the actual library.

The working version used youtubei.js 18.1.0 and Hermes's `static_h` compiler,
pinned to revision
[`7508017ae267ecffe4c4df38656713034f35d9bf`](https://github.com/facebook/hermes/tree/7508017ae267ecffe4c4df38656713034f35d9bf).
It could turn the library into C, and Rust could call the resulting native code.
That was a much more substantial result than adding two numbers.

I kept asking whether the pipeline could get simpler. Did we need esbuild?
Did we need to traverse the whole package? Did we need a handwritten `bridge.c`
just because the previous QuickJS integration had needed a bridge?

The npm package already shipped a self-contained browser bundle under
`youtubei.js/web.bundle`. The pinned Hermes compiler rejected its untouched
ESM export statement, so one ordinary esbuild pass stayed. It combined that
bundle with our facade and host adapters into an IIFE. There were three esbuild
inputs, no custom plugins, no source patches, and no extra target downgrade.
The upstream bundle already contained its own transpilation helpers.

The final build looked like this:

```text
youtubei.js/web.bundle + local JS facade/adapters
                  │
                esbuild
                  │
            JavaScript IIFE
                  │
     shermes -c, plus two callback units
                  │
             object files
                  │
             static archive
                  │
       Rust + linked Hermes runtime
```

Using `shermes -c` removed the separate Clang orchestration from our build
script. Hermes still generated C and invoked the C compiler internally. We
simplified the build without pretending that compilation work had disappeared.
Cargo drove the pipeline; nub and `nub.lock` pinned the JavaScript tooling.

We removed the handwritten C bridge. Small JavaScript/Flow units declared the
native callbacks and drove the selected library operation; Hermes compiled
those too. Rust kept its raw C declarations private and wrapped the runtime in
an owner that released it in `Drop`.

This followed the philosophy I liked in
[rusqlite](https://github.com/rusqlite/rusqlite): put the foreign interface behind
a safe Rust API. A caller should receive values and errors without having to
manage native pointers or garbage-collector roots. The experiment exposed
ordinary calls such as:

```rust
let mut youtube = youtubei_native::Youtube::new()?;
for video in youtube.playlist("PLAYLIST_ID")? {
    println!("{} {}", video.id, video.title);
}
```

That interface was ours to design. Hermes exported compiled units; it did not
automatically turn every youtubei.js method into a convenient C function. The
wrapper supported selected operations, kept the owner on one thread, and
allowed only one live owner. Removing `bridge.c` removed a source file and a
layer of handwritten code. There was still a boundary to maintain.

For host APIs, Rust already had useful implementations. `fetch` used `reqwest`.
URL parsing and query encoding used `url`. Rust handled text conversion, and
Serde handled values crossing the boundary. Small compiled adapters supplied
the request, response, and event-listener shapes the supported operations used.
We did not install a browser-polyfill stack for APIs those operations never
called.

I also wanted to know what happened to `Proxy`, which youtubei.js uses in its
parser helpers. Emitting C still had to preserve those dynamic property reads.

Hermes still handles it. The trap functions become compiled code, while dynamic
property operations use the runtime's object machinery. The same runtime
supplies garbage collection and other JavaScript semantics. The linked support
code is C++ even though the boundary Rust calls is C.

The tested configuration also retains an interpreter and embedded bytecode for
Hermes's own internal JavaScript built-ins. The application and library code
are compiled ahead of time. Those are different claims, and both belong in the
description of the result.

So now I had youtubei.js executing as native code, with a safe Rust wrapper and
no QuickJS. The next question was whether it was any good.

The first live result was awful: roughly **15 seconds per playlist scan and
512 MiB of peak RAM** in a development build. I was ready to call the experiment
a failure and throw it out. Replacing a working QuickJS integration with
something slower and heavier was not the point.

Before abandoning it, we looked at where the time and memory were going.

The HTTP adapter was transferring response bodies as JSON arrays of byte
values. A response became Rust bytes, then JSON numbers, then JavaScript values,
then bytes and text again. For a playlist response whose useful payload was
JSON text, we had built a very expensive round trip.

Returning UTF-8 text directly changed the result dramatically. The development
medians fell to about **4.7 seconds per scan and 71 MiB**. Hermes had not become
a different engine. We had stopped making it process our wasteful transport.

We kept going: reused `reqwest` connections, released retained host buffers
when the owner was dropped, compared release builds, optimized the generated
code, and reduced the initial JavaScript heap from 32 MiB to 8 MiB while keeping
it growable.

Profiling then found another adapter cost: constructing response strings one
character at a time. We replaced that loop with a bulk UTF-8 copy through an
existing Hermes native API. Rust kept the buffer alive until the immediate
copy finished, and an explicit length preserved embedded NULs. The Unicode,
error-recovery, and ownership tests still passed. No new C bridge was needed.

That was enough improvement to deserve a proper comparison with the original
QuickJS integration.

Both runners used youtubei.js 18.1.0. The workload was the complete flat listing
of [Tim Ventura Interviews](https://www.youtube.com/playlist?list=PLipBN7O7_H3oq9oDRWagdZUoBOV81GCkT):
**504 entries at the time of measurement**, including four unavailable
placeholders, through every continuation page. Each implementation matched
the full ID sequence and order from yt-dlp. They also agreed with each other on
titles, durations, and availability. Neither fetched per-video info or
downloaded media.

Each process performed two scans. We alternated engines across three fresh
processes each, using release builds on an Apple Silicon Mac running macOS 26.4
with Rust 1.96. These were standalone executables, without the rest of the
desktop application's dependencies.

The final comparison was:

| Measurement | Original QuickJS integration | Optimized Hermes |
| --- | ---: | ---: |
| First complete scan, including startup | 2.69 s | 2.29 s |
| Second complete scan | 2.25 s | 2.07 s |
| Whole-process CPU time, both scans | 0.86 s | 0.75 s |
| Peak process RAM | 66.4 MiB | 74.8 MiB |
| Executable size | 16.96 MiB | 14.77 MiB |
| Bundle + release runner rebuild, dependencies cached | 4.24 s | 43.75 s |

Hermes ended up with a smaller executable and modestly lower scan and CPU times
in this sample. It still used more RAM, and rebuilding took roughly ten times
as long.

Scan times, CPU time, and peak RAM are medians of the three runs. The rebuild
figures are individual forced builds, including bundling and the Rust runner,
with dependencies and toolchains already installed. QuickJS embeds the
JavaScript source; Hermes also compiles it to machine code. Hermes used its `-O`
optimization with the
native C compiler capped at `-O1`. The native archive pipeline alone took about
43.3 seconds, so the optimized version still fit the one-minute budget. The
earlier fifteen-second build had used unoptimized native code.

These measurements compare the integrations I had actually built. QuickJS kept
its existing async HTTP worker and cached a remotely initialized session.
Hermes used pooled blocking HTTP and generated local sessions without fetching
config or player code. QuickJS also converted listing age labels into
approximate dates; Hermes returned the labels. Three live-network repetitions
cannot isolate the engine's contribution or establish a general performance
winner.

Async HTTP remains worth distinguishing from scan speed. Hermes's blocking
wrapper occupies its owning thread. An async interface could let the caller
do other work while requests are pending. It would not make dependent playlist
pages arrive in parallel: the next request needs the previous response's
continuation token. The C ABI does not inherently forbid async integration;
this prototype did not implement that scheduling boundary.

I published [youtubei-native](https://github.com/meoyawn/youtubei-native) as an
experimental artifact, with the
[benchmark history and methodology](https://github.com/meoyawn/youtubei-native/blob/main/BENCHMARKS.md).
The initial integration failed its performance goal. The optimized version
earned a more interesting conclusion: a working native integration with
measured tradeoffs, without a clear reason to replace QuickJS overall.

It is still a prototype. The verified API covers text parsing, local sessions,
and flat playlists, including modern renderers and continuations. Authentication,
player-script execution, and the rest of the package remain unverified. Promise
jobs use an internal Hermes hook, HTTP blocks, and the build has only been
validated on the tested macOS target. Publishing the experiment makes those
results available; it does not make the crate production-ready.

The next attempt is `youtubei-rust-compiler`.

I want to take one ECMAScript module bundle and generate a Rust crate directly.
Esbuild can bundle youtubei.js and its transitive dependencies while retaining
the exports. The compiler consumes that file. The generated implementation
uses async Rust, `reqwest` for HTTP, `url` for URLs, and Rust libraries for other
host APIs the library actually needs. There is no intermediate C ABI or embedded
JavaScript engine in the intended result.

The scope starts with three concrete source versions: **17.2.0, 18.0.0, and
18.1.0**. The compiler must preserve their exports and behavior. A handwritten
playlist client would leave me maintaining another implementation of YouTube's
API. A compiler lets the upstream package remain the implementation I rebuild.

There is already a frontend using [Oxc](https://oxc.rs/docs/guide/usage/parser.html).
It parses all three complete bundles, resolves lexical bindings, and inventories
their imports, exports, and syntax. Each bundle exposes 46 exports and has no
remaining external or dynamic imports after bundling. Dependencies are present;
esbuild has incorporated them. The analysis takes roughly 190–210 milliseconds
per bundle in the current development build.

**Rust code generation is not implemented yet.** Those numbers measure parsing
and analysis, not compilation to a working Rust library. There is no third
runtime result to put in the comparison table yet.

Generating Rust also will not make JavaScript's semantics disappear. Objects
have identity, mutation, cycles, prototypes, and proxy traps. Closures share
state. Promises have ordering rules. Rust ownership does not automatically
implement JavaScript garbage collection. The compiler will need safe native
support for the behavior these bundles use, as well as generated code. Even
replacing `fetch` requires binding analysis: a local function with that name
must remain a local function.

The acceptance test is concrete. For each of the three versions, generate the
crate, build it, and use its public Rust API to fetch
[this playlist](https://www.youtube.com/playlist?list=PL13A9D0E9048D3941).
Compare the complete flat listing against fresh yt-dlp output: count, IDs,
order, and available listing metadata. Follow every continuation, retain
unavailable placeholders, and never expand individual videos to fill missing
fields. Then run the larger playlist workload again and add the generated Rust
implementation to the same build-time, CPU, RAM, and executable-size comparison.

I still want the boring dependency bump: change the version, rebuild, and call
youtubei.js from Rust. We will get there.
