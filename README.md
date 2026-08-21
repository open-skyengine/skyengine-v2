# SkyEngine v2

SkyEngine v2 is a clean Rust implementation of the MRP application runtime described in
[`docs/design.md`](docs/design.md). The current basic release safely parses MRP containers,
loads V50 and V80 precompiled MR chunks, executes them in the built-in register VM, and renders
MR drawing calls through either a headless RGB565 framebuffer or SDL2.

The default font is `mythroad/system/gb16.uc2`, resolved relative to `--work-dir`. The included
`dsm_gm.mrp` fixture runs through its real `start.mr` loading chain and renders its application-list
UI; there is no package-name dispatch or fixture-specific drawing path.

`--work-dir` represents the device filesystem root. Installed applications and shared runtime
resources use the same layout as a device:

```text
<work-dir>/
  mythroad/
    app.mrp
    system/gb16.uc2
    plugins/*.mrp
```

## Prerequisites

- A current stable Rust toolchain with Cargo
- SDL2 development libraries (`libsdl2-dev` on Debian/Ubuntu)

## Build And Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

To exercise the SDL renderer without a window server:

```bash
SDL_VIDEODRIVER=dummy cargo test -p skyengine-sdl
```

## Inspect A Package

```bash
cargo run -p skyengine -- inspect test/fixtures/dsm_gm.mrp
cargo run -p skyengine -- inspect --json test/fixtures/dsm_gm.mrp
```

`inspect` validates and reports the container without executing guest code.

## Run Headless

```bash
cargo run -p skyengine -- run \
  --headless \
  --work-dir test/fixtures \
  --frame-output skyengine-frame.ppm \
  test/fixtures/mythroad/dsm_gm.mrp
```

The output is a binary P6 PPM image. `--frame-output` implies headless mode. Useful overrides are
`--entry NAME`, `--work-dir DIR`, `--font FILE`, and `--screen WIDTHxHEIGHT`.

## Run With SDL2

```bash
cargo run -p skyengine -- run \
  --work-dir test/fixtures \
  test/fixtures/mythroad/dsm_gm.mrp
```

The SDL window uses a 2x logical scale. Arrow keys map to the MR direction keys, Enter or Space
maps to Select, `F1` and `F2` map to the soft keys, and Escape maps to Back.

## Current Scope

This basic release implements the container reader, precompiled MR chunk frontend, core MR VM,
the standard-library and platform calls needed by the fixture, RGB565 bitmap/sprite/text drawing,
safe work-directory enumeration, and deterministic headless output. Text MR compilation, ARM/Thumb
EXT execution, complete file/network/audio services, and the `skydbg` transport remain later design
milestones. Unsupported formats and platform operations fail explicitly.
