# Infisical's backend at pull request #6995's merge (new) and at the commit before it (old), built the way Infisical builds it, with the
# Kern twin and the proof kit beside it. The build context is twin/.
FROM docker.io/library/node:22.22.0-trixie-slim@sha256:465a8c8f0f4103861bcbcf3e512608394b7155eccb1955425f4ea3f672ddc53e
RUN apt-get update && apt-get install -y --no-install-recommends python3 make g++ git ca-certificates openssl unixodbc unixodbc-dev freetds-bin freetds-dev libc-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /proof
ARG NEW=9d085eba60fe116ba685305704ade7a2a3d8379e
RUN git clone --filter=blob:none --no-checkout https://github.com/Infisical/infisical.git repo \
 && git -C repo checkout --quiet "$NEW" \
 && cd repo/backend && npm ci --no-audit --no-fund && npm run build
COPY package.json /twin/package.json
COPY src /twin/src
COPY bin /twin/bin
COPY proof /twin/proof
RUN node /twin/bin/kern-twin.js build --repo /proof/repo --base "$(git -C /proof/repo rev-parse "$NEW^1")" --out /proof/old --build "cd backend && npm run build" \
 && cp /twin/proof/infisical/twin-entry.mjs /proof/repo/backend/twin-entry.mjs \
 && cp /twin/proof/infisical/twin-entry.mjs /proof/old/backend/twin-entry.mjs \
 && cp /twin/proof/infisical/prepare.cjs /proof/repo/backend/twin-prepare.cjs \
 && cp /twin/proof/infisical/manifest.json /proof/manifest.json
# The database is readied as Infisical's e2e tests ready it (migrations and seed from its TypeScript sources, under its own tsx), then the
# proof server starts in /proof.
CMD ["sh", "-c", "(cd /proof/repo/backend && node --import tsx twin-prepare.cjs) && exec node /twin/proof/kit/server.mjs"]
