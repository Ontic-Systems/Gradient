# Registry backend MVP

Gradient's launch-tier registry backend is a minimal HTTP service for signed package bytes. It stores packages on disk using the same layout as `file://` publishing:

```text
<root>/<package>/<version>/
  <package>-<version>.gradient-pkg
  <package>-<version>.publish.json
  <package>-<version>.sigstore.json
  gradient-package.toml
```

The backend is not a trust root. The trust chain remains in `gradient-package.toml`, the sigstore bundle, and transparency-log metadata.

## Run

```bash
gradient registry-serve --root ./registry --addr 127.0.0.1:7878 --auth-identity <sigstore-identity>
```

`--auth-identity` is optional for local development. When set, every `PUT` must carry:

```text
X-Gradient-Sigstore-Identity: <sigstore-identity>
```

`gradient publish --registry http://127.0.0.1:7878` derives this identity from the generated sigstore bundle and sends it with each upload. The launch implementation prefers the first `tlogEntries[0].uuid`, then `tlog:<logIndex>`, then `logID`.

## Endpoints

### Health

```text
GET /healthz
```

Returns `200 OK` with `ok`.

### Package index

```text
GET /v1/packages/<package>/index.json
```

Returns the versions stored under `<root>/<package>/`:

```json
{
  "schema_version": 1,
  "package": "demo_pkg",
  "versions": ["1.0.0", "1.2.0"]
}
```

Package and version path components are validated with the same safe cache-path rules used by the existing installer/cache code.

### Download package files

```text
GET /v1/packages/<package>/<version>/<filename>
```

Allowed filenames:

- `<package>-<version>.gradient-pkg`
- `<package>-<version>.publish.json`
- `<package>-<version>.sigstore.json`
- `gradient-package.toml`

Other filenames are rejected so requests cannot escape or enumerate the registry root.

### Upload package files

```text
PUT /v1/packages/<package>/<version>/<filename>
X-Gradient-Sigstore-Identity: <identity>
```

Stores one allowed package file at `<root>/<package>/<version>/<filename>`. If the service was started with `--auth-identity`, missing identity returns `401` and mismatched identity returns `403`.

## Publish and install integration

`gradient publish` accepts `file://`, `http://`, and `https://` registries. HTTP(S) upload requires a sigstore bundle, so dry-run publish cannot upload to HTTP. The publish command uploads the artifact, publish metadata, registry manifest, and sigstore bundle to the backend endpoints above.

`gradient install` also accepts `file://`, `http://`, and `https://` registries. For HTTP(S) registries, install downloads the registry manifest, publish metadata, package artifact, and named sigstore bundle through the download endpoint above. The backend remains a byte store: install still verifies manifest/package identity, artifact SHA-256, sigstore transparency-log identity shape, safe archive extraction, and lockfile recording before trusting extracted contents.

## Current limits

- Single-process stdlib HTTP/1.1 server; deploy behind a reverse proxy if exposed beyond localhost.
- No registry-owned signing key; this is intentional for the MVP trust model.
- No federation, yanking, or reserved-name administration yet.
- Install verification remains responsible for validating signatures, manifest trust declarations, and transparency-log metadata before extraction.
