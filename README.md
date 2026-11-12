# Backstack

Backstack is a macOS app for the PostgreSQL backups of multi-tenant applications. It finds
backups on your Mac and in S3-compatible buckets, shows what is inside each one, compares two
of them table by table, and restores one into PostgreSQL (a Docker container or any server)
with a single click.

The scanning, reading and restoring live in a Rust library (`core/`); the SwiftUI app
(`Sources/Backstack`) calls it through a small C interface that exchanges JSON.

The interface is in English and Vietnamese. It follows the system language, and can be
switched in the settings (⌘,) without restarting.

## Contents

- [Features](#features)
- [Backup formats](#backup-formats)
- [S3 buckets](#s3-buckets)
- [Restore targets](#restore-targets)
- [Safety](#safety)
- [Install](#install)
- [Build from source](#build-from-source)
- [Testing](#testing)
- [How it works](#how-it-works)
- [Limitations](#limitations)
- [License](#license)

## Features

- **Find.** Scans the folders you choose (`~/Downloads` by default) and any number of
  S3-compatible buckets. A backup folder, its `.tar.gz` archive and the archive's copy in S3
  are shown as one backup.
- **Organize.** Groups backups by tenant, newest first, with the company name, app version
  and source database when the backup records them.
- **Inspect.** Reads `database.sql` straight out of a folder or archive, without unpacking
  anything: every table with its row count, the PostgreSQL version of the dump, and how many
  uploaded files the backup carries. A 1.3 GB dump is read in under half a second.
- **Compare.** Puts two backups of the same tenant side by side: rows per table before and
  after, tables added and dropped, and a warning for any table that lost all its rows.
- **Restore.** Streams the dump into a new database with a progress bar and a stop button.
  Restores from S3 are streamed too, and the download stops as soon as `database.sql` has
  been read.
- **Name databases your way.** Restored databases are named from a template such as
  `{tenant}_{date}` or `myapp_{tenant}_{date}`, and the name can be edited before each
  restore.
- **Clean up.** Lists the databases on the current target with their size so old restores
  can be dropped, and moves older local backups to the Trash, keeping the newest N per tenant.

## Backup formats

Backstack recognizes three shapes of backup:

| Shape | What it contains |
| --- | --- |
| Folder | `database.sql` (a plain-SQL `pg_dump`), an optional `metadata.json`, and an optional `files/` folder of uploads |
| Archive | The same folder as `.tar.gz` or `.tgz`, with `metadata.json` and `database.sql` as its first members |
| Dump | A `pg_dump` custom-format `.dump` file |

A name that ends in a timestamp gives the tenant and the date, for example
`shop_2026-11-09T02-08-46-592Z` or `shop_20260921-160651`. When there is a `metadata.json`,
these members are shown:

```json
{
  "tenant": "shop",
  "timestamp": "2026-11-09T02:09:19Z",
  "includes": ["database", "files"],
  "config": {
    "name": "Shop Ltd",
    "slug": "shop",
    "dbName": "shop_db",
    "appVersion": "1.6.0"
  }
}
```

Every member is optional. Anything else in the file is ignored (see [Safety](#safety)).

## S3 buckets

Add a bucket in the S3 tab of the settings with its endpoint, port, TLS, region, bucket name,
optional prefix and access keys. Any S3-compatible service works: AWS S3, Backblaze B2,
Wasabi, Aliyun OSS, MinIO and others.

Backups are expected at:

```
{prefix}{tenant}/{file}
```

Folders whose name starts with `_` are skipped, so a bucket can hold other data beside the
tenants. While scanning, only the first 256 KB of each archive is downloaded to read its
`metadata.json`. Reading the full table list of a backup that exists only in S3 means
downloading it, so the app does that only when asked.

The bucket editor can list the bucket to confirm the settings before saving.

## Restore targets

Targets are kept in the Postgres tab of the settings and picked from the toolbar.

**Docker container.** Commands run through `docker exec`. On first launch Backstack looks
for running containers whose image is PostgreSQL (`postgres`, `postgis`, `timescale`,
`pgvector`) and adds each one, putting containers that publish a port first.

**PostgreSQL server.** Any host and port, with user, password and SSL mode. Backstack uses
the `psql` and `pg_restore` installed on the Mac, from Homebrew (`brew install libpq`) or
Postgres.app.

Plain-SQL dumps are restored with `psql -v ON_ERROR_STOP=1`; custom-format dumps with
`pg_restore --no-owner --no-privileges --exit-on-error`.

Database names use the template set in the settings:

| Placeholder | Value |
| --- | --- |
| `{tenant}` | The tenant name |
| `{date}` | The backup date, `yyyymmdd` |
| `{time}` | The backup time, `hhmm` |

The result is lowercased and reduced to letters, digits and `_`, so PostgreSQL accepts it
without quoting. With `myapp_{tenant}_{date}`, tenant `shop` becomes `myapp_shop_20261109`,
and a tenant already named `myapp_shop` does not get the prefix twice.

## Safety

- **Servers are create-only by default.** A server target refuses to drop or overwrite a
  database unless dropping is explicitly allowed in that server's settings.
  The Rust core enforces this, so pointing Backstack at staging or production can only ever
  create new databases.
- **Failed restores clean up.** A restore that fails or is stopped drops the database it was
  creating, so no half-restored database is left behind.
- **Buckets are read-only.** Backstack never writes to or deletes from a bucket.
- **Secrets stay in the Keychain.** S3 secret keys and server passwords are stored in the
  macOS Keychain, not in preferences. Passwords reach `psql` through its environment, never on
  the command line.
- **Backup credentials are never read.** Only the `metadata.json` members listed above are
  parsed. Credentials a backup may carry are never read into memory as values, shown, or sent
  anywhere.
- **Trash, not delete.** Cleaning up old local backups moves them to the Trash. Copies in S3
  are never touched.
- **Database names are validated.** Names are checked against
  `^[a-z_][a-z0-9_]{0,62}$` before they reach PostgreSQL, and system databases cannot be
  dropped.

## Install

Download the DMG from the releases, open it, and drag **Backstack** to Applications.

Builds are not notarized. The first time, right-click the app and choose **Open**, or allow
it under **System Settings → Privacy & Security**.

Requirements:

- macOS 14 or later on Apple Silicon
- Docker, for container targets
- `psql` and `pg_restore` (`brew install libpq` or Postgres.app), for server targets

## Build from source

Requires the Xcode command line tools and a Rust toolchain.

```sh
./build.sh                # runs the Rust tests, builds release, creates build/Backstack.app
open build/Backstack.app
./dmg.sh                  # creates build/Backstack-<version>.dmg
```

To redraw the icon, run `swift scripts/make-icon.swift`.

The core can also be used without the app:

```sh
# List the backups in a folder and inspect the largest one
cargo run --release --manifest-path core/Cargo.toml --example scan [folder]

# Scan a bucket, inspect its largest backup, restore it into a throwaway database, drop it
S3_ENDPOINT=host:port S3_BUCKET=… S3_PREFIX=… S3_KEY=… S3_SECRET=… PG_CONTAINER=… \
  cargo run --release --manifest-path core/Cargo.toml --example s3_check

# Restore one backup into a throwaway database and drop it again
cargo run --release --manifest-path core/Cargo.toml --example restore_check -- \
  <path> <folder|archive|dump> <container>
```

## Testing

```sh
cargo test --manifest-path core/Cargo.toml
```

The integration tests are ignored by default because they need running services. They
create and drop throwaway `backstack_*` databases, and the S3 test creates and removes a
scratch bucket:

```sh
BACKSTACK_TEST_CONTAINER=postgres \
BACKSTACK_TEST_PG=localhost:5432 BACKSTACK_TEST_PG_PASSWORD=… \
BACKSTACK_TEST_S3=localhost:9000 BACKSTACK_TEST_S3_KEY=… BACKSTACK_TEST_S3_SECRET=… \
  cargo test --manifest-path core/Cargo.toml -- --include-ignored
```

## How it works

```
core/src/
  scan.rs     finds folders, archives and dumps on disk and in buckets, groups their copies
  inspect.rs  counts the rows of every COPY block in database.sql, and the uploaded files
  s3.rs       lists and streams objects through presigned URLs (rusty-s3 + ureq), no SDK
  source.rs   one copy of a backup: a local path or s3://bucket/key
  pg.rs       psql / pg_restore through docker exec or locally; restores as background jobs
  stamp.rs    timestamps in backup names
  lib.rs      the C functions the app calls, each taking and returning JSON
Sources/Backstack/
  Core.swift  the JSON types, the Keychain, and the calls into Rust
  Store.swift app state and settings
  Views.swift the SwiftUI interface
```

Archives are never unpacked to disk. To restore one, the core reads tar headers from the
gzip stream until it reaches `database.sql`, then pipes that member alone into `psql`.

## Limitations

- Only the database is restored. The `files/` folder of uploads is not copied anywhere.
- Plain-SQL dumps show their tables and row counts. Custom-format `.dump` files are restored,
  but their contents are not listed.
- Builds are for Apple Silicon and are not notarized.

## License

[MIT](LICENSE)
