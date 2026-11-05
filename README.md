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

With Ghostwrite's [`scripts/plugin-build.sh`](https://github.com/ghostwrite-dev/ghostwrite/blob/plugin-api-v1.0.1/scripts/plugin-build.sh):

```sh
git clone --depth 1 --branch plugin-api-v1.0.1 https://github.com/ghostwrite-dev/ghostwrite ../ghostwrite
rustup target add wasm32-wasip2
sh ../ghostwrite/scripts/plugin-build.sh
```

That makes `target/plugin/typst`, which `:plugin install target/plugin/typst` installs. It's built
with Rust 1.97.0, against [`ghostwrite-plugin-api`](https://github.com/ghostwrite-dev/ghostwrite/tree/plugin-api-v1.0.1/crates/plugin-api)
at the tag `plugin-api-v1.0.1`, and with the versions in `Cargo.lock`.

## Releases

A tag `v<version>`, the version in `plugin.json`, releases it as `typst-<version>.zip`, with its
sha256 in `typst-<version>.zip.sha256`.

## License

Mozilla Public License 2.0: see [LICENSE](LICENSE).
