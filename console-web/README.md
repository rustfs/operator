# RustFS Operator Console Web

Frontend for the RustFS Operator Console (login, dashboard, tenant management). Built with Next.js and designed to run in Kubernetes next to the console backend.

## Development

```bash
pnpm install
pnpm dev
```

Open [http://localhost:3000](http://localhost:3000). The app calls the console API at `http://localhost:9090` in dev only if you set the env below; by default it uses relative `/api/v1` (see Deployment).

### Local dev with backend

Run the operator console backend (e.g. `cargo run -- server` or another port). Then either:

- Use same-origin: e.g. put frontend and backend behind one dev server that proxies `/api/v1` to the backend, and run the frontend with `NEXT_PUBLIC_API_BASE_URL=` (empty or `/api/v1`), or
- Use different ports: run frontend on 3000, backend on 9090, and set `NEXT_PUBLIC_API_BASE_URL=http://localhost:9090/api/v1`. The backend allows `http://localhost:3000` by default (CORS).

## Build

```bash
pnpm build
```

Static output is in `out/`. The default API base URL is **`/api/v1`** (relative), so the same build works when the app is served under the same host as the API (e.g. Ingress with `/` → frontend and `/api` → backend).

## Deployment (Kubernetes)

When frontend and backend are deployed in the same cluster and exposed under **one host** (recommended):

1. Build the Docker image (from repo root):

   ```bash
   docker build -t your-registry/console-web:latest console-web/
   ```

2. Enable the console frontend in the Helm chart and Ingress (see [deploy/rustfs-operator/README.md](../deploy/rustfs-operator/README.md#console-ui-frontend--backend-in-k8s)). The Ingress will serve `/` from this app and `/api` from the backend.

3. Do **not** set `NEXT_PUBLIC_API_BASE_URL` (or set it to `/api/v1`). The browser will send requests to the same origin, so cookies and CORS work without extra config.

If the frontend is served from a **different host** than the API, set at build time:

```bash
NEXT_PUBLIC_API_BASE_URL=https://api.example.com/api/v1 pnpm build
```

Then configure the backend with `CORS_ALLOWED_ORIGINS` (see deploy README).

## Environment variables

| Variable                   | Description                             | Default     |
| -------------------------- | --------------------------------------- | ----------- |
| `NEXT_PUBLIC_BASE_PATH`    | Base path for the app (e.g. `/console`) | `""`        |
| `NEXT_PUBLIC_API_BASE_URL` | API base URL (relative or absolute)     | `"/api/v1"` |
