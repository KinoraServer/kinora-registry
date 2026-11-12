# Registry format 1

A registry is a folder of static files. Any web server can host one.

```text
index.json                              every package and its versions
index.json.sig                          Ed25519 signature of index.json (optional)
packages/<id>/<version>/package.json    the manifest of one version
packages/<id>/<version>/files/...       its files
```

Kinora downloads `index.json` to show the list, and a version's manifest and files only when
an admin installs it. JSON Schemas for the files are in [`schema/`](../schema).

- [The source `package.json`](#the-source-packagejson)
- [What each kind holds](#what-each-kind-holds)
- [The published manifest](#the-published-manifest)
- [`index.json`](#indexjson)
- [Signing](#signing)
- [Versions never change](#versions-never-change)
- [Your own registry or a mirror](#your-own-registry-or-a-mirror)

## The source `package.json`

One per folder in `packages/`. The folder is named after the id.

| Field         | Required | What                                                                   |
| ------------- | -------- | ---------------------------------------------------------------------- |
| `id`          | yes      | Lowercase letters, digits and dashes, at most 64 characters             |
| `kind`        | yes      | `module`, `theme`, `player-skin` or `app-skin`                          |
| `name`        | yes      | Shown in the list, at most 60 characters                                |
| `description` | yes      | One or two sentences, at most 300 characters                            |
| `version`     | yes      | `x.y.z`                                                                 |
| `author`      | yes      | Who made it                                                             |
| `license`     | yes      | An SPDX id, e.g. `GPL-2.0-only`                                         |
| `homepage`    | no       | An `https://` link                                                      |
| `recommended` | no       | `true` puts a "Recommended" mark on it                                  |
| `min_kinora`  | no       | The oldest Kinora version that can run it. Older servers don't offer it |
| `module`      | modules  | See [modules.md](modules.md#packagejson)                                |

## What each kind holds

| Kind          | In `files/`                                                    | Installed to                   |
| ------------- | -------------------------------------------------------------- | ------------------------------ |
| `theme`       | One or more `<theme id>.json` theme files, nothing else        | `<config>/themes/`             |
| `player-skin` | `skin.json`, `skin.html` and optionally `skin.css`             | `<config>/player-skins/<id>/`  |
| `app-skin`    | `skin.json` (same `id` and `version` as the package) and the skin's templates, CSS and assets | `<config>/app-skins/<id>/` |
| `module`      | Nothing: `module.wasm` is built from the package's Rust source | `<config>/modules/<id>/`       |

Limits for every package: at most 300 files, 4 MB per file and 20 MB in total. File names use
letters, digits, `.`, `_` and `-`, and don't start with a dot.

The build runs the same checks Kinora runs on install, so a package that builds here installs
there. A theme can't use the ids `kinora` or `kinora-light`, and two packages can't carry a
theme with the same id.

## The published manifest

`packages/<id>/<version>/package.json` is the source `package.json` plus what the build adds:

```json
{
  "format": 1,
  "id": "ember",
  "kind": "theme",
  "name": "Ember",
  "description": "A warm dark theme with an orange accent and rounder corners.",
  "version": "1.0.0",
  "author": "Kinora",
  "license": "GPL-2.0-only",
  "files": [{ "path": "ember.json", "size": 612, "sha256": "…" }],
  "source_sha256": "…",
  "published_at": 1794441686049
}
```

- `files`: every file under `files/`, with its size in bytes and SHA-256.
- `source_sha256`: a hash of the package's source folder. The build uses it to notice a change
  without a new version.
- `published_at`: milliseconds since 1970.
- For a module, `module.crate` is replaced by `"entry": "module.wasm"`.

## `index.json`

```json
{
  "format": 1,
  "packages": [
    {
      "id": "tmdb",
      "kind": "module",
      "name": "TMDB metadata",
      "description": "…",
      "author": "Kinora",
      "license": "GPL-2.0-only",
      "recommended": true,
      "provides": ["metadata_provider"],
      "permissions": { "http": ["api.themoviedb.org"] },
      "versions": [
        {
          "version": "1.0.0",
          "abi": 1,
          "size": 236541,
          "published_at": 1794441686049,
          "manifest": "packages/tmdb/1.0.0/package.json",
          "sha256": "…"
        }
      ]
    }
  ]
}
```

Versions are listed newest first; the name, description and permissions are the newest
version's. `sha256` is the hash of that version's manifest. `manifest` is always
`packages/<id>/<version>/package.json`: Kinora refuses an index that points anywhere else.
A server installs the newest version whose `min_kinora` and `abi` it supports.

A server that doesn't know `format` refuses the index and tells the admin to update Kinora.

## Signing

`index.json.sig` is an Ed25519 signature over the exact bytes of `index.json`:

```json
{ "algorithm": "ed25519", "key_id": "0123456789abcdef", "signature": "<base64>" }
```

`key_id` is the first 16 hex characters of the SHA-256 of the public key. A server that has
public keys for a registry refuses an index that isn't signed by one of them. From there the
chain is hashes: the index lists each manifest's SHA-256, and each manifest lists each file's.

`node scripts/keygen.mjs` makes a key pair. The build signs when the environment variable
`REGISTRY_SIGNING_KEY` holds the private key (the base64 32-byte seed).

## Versions never change

`node scripts/build.mjs --previous <folder>` builds on top of what is already published:
existing versions are copied as they are and only new versions are built. If a package's
source changed but its version is already published, the build fails and asks for a new
version. This is how the publish workflow runs it, so an installed package can always be
downloaded again byte for byte.

To take a package out of the list, delete its folder from `packages/` and its folder from the
`published` branch. Servers that have it installed keep it.

## Your own registry or a mirror

Copy the published files (or build your own packages with these scripts) to any web server
and start Kinora with:

| Variable              | What                                                            |
| --------------------- | --------------------------------------------------------------- |
| `KINORA_REGISTRY_URL` | The folder that holds `index.json`                              |
| `KINORA_REGISTRY_KEY` | Your public key(s), comma separated, when your index is signed  |

A server reads one registry. A straight copy of this registry keeps its signature valid,
because the signature covers the bytes of `index.json` and not where it is hosted.
