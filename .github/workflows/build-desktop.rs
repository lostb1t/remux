name: Build Dioxus Desktop

on:
  workflow_dispatch:

jobs:
  build:
    runs-on: ${{ matrix.runner }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: linux
            runner: ubuntu-latest
            target: x86_64-unknown-linux-gnu

    steps:
      - name: Checkout repo
        uses: actions/checkout@v4

      - name: Set up Rust
        uses: actions-rs/toolchain@v1
        with:
          profile: minimal
          toolchain: stable
          override: true

      - name: Install Dioxus CLI
        run: cargo install dioxus-cli@0.7.0-alpha32

      - name: Install dependencies (Linux only)
        if: matrix.os == 'linux'
        run: sudo apt-get update && sudo apt-get install -y libx11-dev libgtk-3-dev libwebkit2gtk-4.0-dev

      - name: Build Dioxus desktop app
        run: dx build --release --platform desktop

      - name: Upload build artifact
        uses: actions/upload-artifact@v4
        with:
          name: remux-${{ matrix.os }}
          path: target/release