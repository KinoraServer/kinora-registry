# Kinora registry

The packages a [Kinora](https://github.com/KinoraServer/kinora) server can install at runtime
from **Admin → Modules & themes → Add modules, themes and skins**: modules, themes, player
skins and app skins. Nothing here is part of the server until an admin presses Install.

| Package                  | Kind        | What                                                              |
| ------------------------ | ----------- | ----------------------------------------------------------------- |
| `tmdb`                   | module      | Titles, descriptions and artwork from The Movie Database          |
| `auto-tracks`            | module      | Remembers each viewer's audio and subtitle tracks per show         |
| `activity-log`           | module      | A history of playback, scans and new users in Admin → Activity     |
| `midnight`               | theme       | Midnight (dark) and Daylight (light), a cool blue-grey pair        |
| `oled`                   | theme       | OLED Black                                                         |
| `ember`                  | theme       | A warm dark theme with an orange accent                            |
| `neon`                   | player skin | A glowing look for the video player                                |
| `marquee`                | app skin    | A cinematic look for the whole app                                 |

## How it works

Each folder in `packages/` is one package: a `package.json` that describes it, and either a
`files/` folder (themes and skins) or Rust source that is compiled to one sandboxed
WebAssembly file (modules).

`node scripts/build.mjs` checks every package and writes the registry into `dist/`: plain
static files with an `index.json` on top. On every push to `main` the publish workflow builds
it and pushes the result to the `published` branch, which is what Kinora servers read:

```text
https://raw.githubusercontent.com/KinoraServer/kinora-registry/published/index.json
```

A published version never changes. To change a package, raise its `version`; older versions
stay available. The index is signed, and every file is listed with its SHA-256, so a server
can tell that what it downloaded is what was published here.

- [docs/format.md](docs/format.md): the registry's files, `package.json`, signing, and how to
  host your own registry or a mirror.
- [docs/modules.md](docs/modules.md): writing a module.

## Working on packages

You need Node 22 or newer. For modules you also need Rust with the WebAssembly target:

```sh
rustup target add wasm32-unknown-unknown
```

```sh
node scripts/build.mjs --check   # check every package.json and its files
cargo test --workspace           # the modules' unit tests
node scripts/build.mjs           # build everything into dist/
```

To try your build in a Kinora server, serve `dist/` and point the server at it:

```sh
python3 -m http.server 9911 --bind 127.0.0.1 --directory dist
KINORA_REGISTRY_URL=http://127.0.0.1:9911 kinora
```

### Adding a theme or a skin

1. Make `packages/<id>/` with a `package.json` (copy one from a package of the same kind).
2. Put the files in `packages/<id>/files/`, the same files you would put in the server's
   `themes/`, `player-skins/<id>/` or `app-skins/<id>/` folder. Kinora's own docs describe
   them: `docs/themes.md` and `docs/app-skins.md` in the Kinora repository.
3. Run `node scripts/build.mjs --check` and open a pull request.

### Changing a package

Raise `version` in its `package.json` (and in `Cargo.toml` for a module, and in `skin.json`
for an app skin). The build refuses a change to a version that is already published.

## License

GPL-2.0-only, like Kinora. See [LICENSE](LICENSE). A package states its own license in its
`package.json`.
