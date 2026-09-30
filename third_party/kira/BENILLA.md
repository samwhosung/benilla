# benilla's `kira` fork — what differs from upstream, and how to check

Upstream: [`tesselode/kira`](https://github.com/tesselode/kira), MIT OR Apache-2.0, version
`0.12.4` — the version the workspace lock resolves to. Wired in through `[patch.crates-io]` in
the workspace root, the way `lua-src` is. The crate is copied from the cargo registry as
published (`src/`, `Cargo.toml`, `README.md`) plus the two licence files from the repository;
upstream files are CRLF and are left so, except the two files patched, which are LF.

0.12.4 is the first kira on symphonia 0.6, whose WAV reader sizes a PCM frame from the channels
and sample width. 0.5 took it from the header's `nBlockAlign`, which 98 of the install's stereo
WAVs store as 2, a mono frame, so it counted them at twice their length and a looping stream of
one fell silent at the real end. 0.6's MP3 decoder also matches mpg123 within 6e-6 on 199 of the
install's 200 MP3s and 1.4e-3 on the last, where 0.5's is off by more than 0.01, up to 0.15 of
full scale, in some frames of 63; 0.6 ends 83 of them one silent MPEG frame (1152 samples)
sooner than both.
0.12.5 differs from 0.12.4 only in taking `rtrb` 0.4, which `benilla-app` shares with kira.

## Why a fork exists at all

The mixer runs on benilla's own audio render thread (`sound/output`), and with a raid buffing
itself — a dozen live voices, most of them spatial — that thread cost **~1.5 ms of every 60 Hz
frame** on an M2 Air (a sampled profile with a dozen spatial voices playing, `Track::process`
two thirds of it). Upstream's sub-track `process` spatializes **every
frame**: two vector normalizes, a length, the attenuation curve and a dB→amplitude `powf` per
frame per spatial track, then two more `powf`s per frame for the volume tween and the fade. At
48 kHz × 12 tracks that is ~1.7 M `powf`s a second plus the vector math, for gains that change
imperceptibly across a 10 ms chunk. FMOD 3 — the mixer the real client delegates to — updates
pan and attenuation per mixer update, not per sample.

## Patch 1 — `src/track/sub.rs`

`Track::process` evaluates the spatial gains (attenuation, left/right ear gain, the mono fold)
and the volume × fade amplitude at the chunk's two ends and interpolates them linearly across
the chunk. `SpatialData::spatial_gains` is `spatialize` factored into a value; `spatialize`
itself is kept, unused, so the factoring can be diffed against it. A chunk is the output
device's buffer — 512 frames, ~10.7 ms, `sound/output::DEVICE_BUFFER_FRAMES`.

What changes audibly: nothing a listener can name. A source crossing an ear's axis within one
chunk pans linearly in gain over 10 ms instead of following the dot product per sample; a
volume tween ramps linearly in amplitude across each chunk instead of in decibels. Both are
below the granularity the reference itself updates at.

## Patch 2 — `src/sound/streaming/decoder/symphonia.rs`

`SymphoniaDecoder::decode` returns an error at the end of the packets, as 0.12.1 did on
symphonia 0.5, where 0.12.4 returns an empty chunk. `DecodeScheduler::frame_at_index` decodes
until it holds the frame asked for, so when a stream's reported length runs past its last packet
it loops on empty chunks forever: the sound never ends and its decode thread spins until the
process exits, even after the sound is stopped. symphonia 0.6 estimates the length of an MP3
without a Xing header from its first frames, and over-counts one of the install's tracks,
`ZulGurubVooDoo.mp3`, by one frame. With the error the sound stops where its packets end.

## How to check

For each patched file, `diff -u <(sed 's/\r$//' <registry>/kira-0.12.4/<file>)
third_party/kira/<file>` shows the patch and nothing else; every other file is byte-identical to
the registry crate. The mixer's own tests (`cargo test -p benilla-app sound::`) run against the
fork, `sound::mixer::tests::a_stream_that_runs_out_before_its_reported_length_stops` on patch 2.
