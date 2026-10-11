# firecrawl's API at pull request #2638's merge (new) and at the commit before it (old), built the way firecrawl builds it (pnpm, and
# Rust for its native module), with the Kern twin and the proof kit beside it. The build context is twin/.
FROM docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392
RUN apt-get update && apt-get install -y --no-install-recommends curl build-essential pkg-config python3 git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH CI=true
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal && corepack enable
WORKDIR /proof
ARG NEW=449a5d612ccf8e9ca20d746c83483d62ceb78c24
RUN git clone --filter=blob:none --no-checkout https://github.com/firecrawl/firecrawl.git repo \
 && git -C repo checkout --quiet "$NEW" \
 && cd repo/apps/api && pnpm install --frozen-lockfile && pnpm run build
COPY package.json /twin/package.json
COPY src /twin/src
COPY bin /twin/bin
COPY proof /twin/proof
RUN node /twin/bin/kern-twin.js build --repo /proof/repo --base "$(git -C /proof/repo rev-parse "$NEW^1")" --out /proof/old --build "cd apps/api && pnpm run build" \
 && cp /twin/proof/firecrawl/twin-entry.mjs /proof/repo/apps/api/twin-entry.mjs \
 && cp /twin/proof/firecrawl/twin-entry.mjs /proof/old/apps/api/twin-entry.mjs \
 && cp /twin/proof/firecrawl/manifest.json /proof/manifest.json
CMD ["node", "/twin/proof/kit/server.mjs"]
