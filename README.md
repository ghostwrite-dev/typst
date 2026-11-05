# Typst for Ghostwrite

A [Ghostwrite](https://github.com/ghostwrite-dev/ghostwrite) plugin for [Typst](https://typst.app):
`.typ` files, compiled as you type, their pages in a tab (`:preview` or `space v`, and back),
colored by Typst's own syntax, and served by [`tinymist`](https://github.com/Myriad-Dreamin/tinymist)
if it's on your PATH.

Its code is Typst itself, built for WebAssembly. It reads the project and the system's fonts, keeps
Typst's packages in Typst's own folders, and downloads them from packages.typst.org and nowhere
else, as `"access"` in its `plugin.json` says.

## Installing

In Ghostwrite:

```
:plugin install ghostwrite-dev/typst
```

That installs its latest release.

## Building

With [plugin-build](https://github.com/ghostwrite-dev/plugin-build)'s `build.sh`:

```sh
git clone https://github.com/ghostwrite-dev/plugin-build ../plugin-build
rustup target add wasm32-wasip2
sh ../plugin-build/build.sh
```

That makes `target/plugin/typst`, which `:plugin install target/plugin/typst` installs. It's built
with Rust 1.97.0, against [`ghostwrite-plugin-api`](https://github.com/ghostwrite-dev/ghostwrite/tree/plugin-api-v2.0.0/crates/plugin-api)
at the tag `plugin-api-v2.0.0`, and with the versions in `Cargo.lock`.

## Releases

A tag `v<version>`, the version in `plugin.json`, releases it as `typst-<version>.zip`, with its
sha256 in `typst-<version>.zip.sha256`.

## License

MIT: see [LICENSE](LICENSE).
