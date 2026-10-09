# Licensing and codecs

## This project

Mist Remote Play is licensed under **MIT OR Apache-2.0** (`LICENSE-MIT`,
`LICENSE-APACHE`). All code, the UI and the icon are original.

## Rust dependencies

Audited with `cargo metadata` (October 2026, ~670 crates): all are
permissively licensed (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0,
Unlicense, CC0, BSL-1.0) except five **MPL-2.0** crates used unmodified:
`cssparser`, `cssparser-macros`, `dtoa-short`, `selectors` (pulled in by
Tauri/WebKit tooling) and `option-ext` (via `directories`). MPL-2.0 is file-level
copyleft: shipping them unmodified in binaries is fine; modifications to
those files would have to be published. Re-run the audit before a release:

```bash
cargo metadata --format-version 1 | python3 -c "import json,sys,collections; m=json.load(sys.stdin); print(collections.Counter(p.get('license') for p in m['packages'] if p['source']))"
```

(`cargo deny check licenses` is a good CI addition; listed in TASKS.md.)

## npm dependencies (desktop UI)

React, React DOM (MIT), `@tauri-apps/api` (MIT/Apache-2.0); build-time only:
Vite, Tailwind CSS, TypeScript, Vitest (MIT/Apache-2.0).

## GStreamer and plugins (runtime, dynamically linked)

| Component | License | Note |
|---|---|---|
| GStreamer core, base, good, bad | LGPL-2.1+ | dynamic linking; users can replace the libraries |
| libnice | LGPL-2.1 / MPL-1.1 | WebRTC ICE |
| `x264enc` (gst-plugins-ugly + libx264) | **GPL-2.0+** | Distributing an application *bundled* with x264 makes the bundle subject to the GPL. Mist works without it (OpenH264 or hardware encoders). |
| `openh264` | BSD-2 | Cisco's prebuilt binaries include a patent license for H.264 when downloaded from Cisco |
| `avdec_h264` (gst-libav / FFmpeg) | LGPL-2.1+ (if built without GPL parts) | |
| Hardware codec plugins (nvcodec, va, qsv, amf, mediafoundation, d3d11) | LGPL-2.1+ | vendor drivers have their own licenses |

**Patents**: H.264 is covered by patents in some jurisdictions (MPEG LA /
Via LA pool). Hardware encoders and OS codecs are usually licensed through
the vendor. When distributing binaries that include a software H.264 encoder
or decoder, check the requirements for your jurisdiction. AV1 is a
royalty-free alternative listed in TASKS.md.

Mist's Linux packages depend on the distribution's GStreamer packages rather
than bundling them, so the codec choice (and its license) stays with the
distribution and the user.

## Trademarks

"Steam", "Steam Link", "Moonlight", "Xbox", "PlayStation" and other names
are trademarks of their owners and are used only descriptively. Virtual
gamepads on Linux report the USB vendor/product IDs of a common wired
Xbox-style controller for driver/SDL compatibility; their device name is
"Mist Virtual Gamepad".
