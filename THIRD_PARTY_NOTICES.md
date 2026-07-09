# Third-Party Notices

Postretro's own source code is licensed under the PolyForm Noncommercial License
1.0.0 (see [`LICENSE`](./LICENSE)), with commercial licensing available
separately (see [`LICENSE-COMMERCIAL.md`](./LICENSE-COMMERCIAL.md)).

Postretro also incorporates and depends on third-party open-source software.
Those components remain under **their own licenses**, which are unaffected by
Postretro's licensing. Postretro's noncommercial/commercial split applies only
to Postretro's own code — it does not, and cannot, relicense these dependencies.
Downstream users receive the third-party components under their original terms.

## License landscape of the dependency tree

The resolved dependency graph (~620 crates) consists almost entirely of
permissive licenses, plus a small number of weak, file-level copyleft
components. No strong copyleft (GPL/LGPL/AGPL) obligations apply: the only
GPL/LGPL strings in the tree appear inside `OR` license expressions whose
permissive branch is selected (e.g. `self_cell`, `r-efi`).

Licenses present in the tree:

- **Permissive:** MIT, Apache-2.0 (incl. Apache-2.0 WITH LLVM-exception),
  BSD-2-Clause, BSD-3-Clause, ISC, Zlib, CC0-1.0, Unicode-3.0, BSL-1.0,
  Unlicense.
- **Weak / file-level copyleft:** MPL-2.0.

### MPL-2.0 components

The following crates are licensed under the Mozilla Public License 2.0
(`MPL-2.0` / `MPL-2.0+`):

- The **`symphonia`** audio-decoding stack, pulled in by `kira` for OGG/Vorbis
  and WAV/PCM decoding: `symphonia`, `symphonia-core`, `symphonia-codec-pcm`,
  `symphonia-codec-vorbis`, `symphonia-format-ogg`, `symphonia-format-riff`,
  `symphonia-metadata`, `symphonia-utils-xiph`.
- **`option-ext`** (via `directories`).
- **`smartstring`**.
- **`triple_buffer`**.

MPL-2.0 is weak, file-scoped copyleft: it does not restrict commercial use and
does not extend to Postretro's own files. Its only obligation, when the covered
files are *modified*, is to make the source of those files available under
MPL-2.0. Postretro consumes these crates unmodified, so the practical
requirement is simply that their source remains available (it is published on
crates.io and the projects' repositories).

## Attribution obligations when distributing binaries

Permissive licenses (MIT, BSD, ISC, Zlib) require preserving their copyright and
permission notices. Apache-2.0 additionally requires including the license,
propagating any `NOTICE` file, and stating significant changes. These duties are
compatible with Postretro's licensing but must be satisfied when you distribute
builds.

## Generating a complete per-crate manifest

This file summarizes the license landscape; it is **not** a substitute for a
full per-crate attribution manifest. Before distributing binaries, generate the
complete list (with each crate's version, license, and notice text) using one of:

```bash
cargo install cargo-about       && cargo about generate about.hbs > licenses.html
# or
cargo install cargo-bundle-licenses && cargo bundle-licenses --format yaml --output THIRD_PARTY_LICENSES.yaml
```

and ship the generated output alongside the binary.
