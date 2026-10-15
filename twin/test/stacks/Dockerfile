ARG NODE=docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392
FROM ${NODE}
WORKDIR /stacks
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
