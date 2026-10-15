FROM docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392
WORKDIR /shop
COPY examples/shop/package.json examples/shop/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY package.json /twin/package.json
COPY src /twin/src
COPY examples/shop/old ./old
COPY examples/shop/new ./new
COPY examples/shop/server.js examples/shop/traffic.js ./
CMD ["node", "server.js"]
