# Build stage: produce static export (out/)
FROM node:22-alpine AS builder

WORKDIR /app

# Pin pnpm to package.json "packageManager" (avoid corepack fetching pnpm@latest from npm;
# that fetch can fail behind proxies / flaky TLS during docker build).
ARG PNPM_VERSION=10.28.1
RUN npm install -g pnpm@${PNPM_VERSION}

COPY package.json pnpm-lock.yaml* pnpm-workspace.yaml* ./
RUN pnpm install --frozen-lockfile

COPY . .
# Same-origin: default /api/v1. For local port-forward dev use: --build-arg NEXT_PUBLIC_API_BASE_URL=http://localhost:9090/api/v1
ARG NEXT_PUBLIC_API_BASE_URL=
ENV NEXT_PUBLIC_API_BASE_URL=${NEXT_PUBLIC_API_BASE_URL}
# If build needs internet (e.g. next/font/google), run: docker build --network=host .
RUN pnpm build

# Run stage: nginx serves static files
FROM nginx:alpine

# SPA: try_files for client-side routes; Next export emits per-route index.html
COPY --from=builder /app/out /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf

RUN chown -R nginx:nginx /usr/share/nginx/html

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
