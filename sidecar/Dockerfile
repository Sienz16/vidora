FROM node:24.22.3-trixie-slim@sha256:af02d57b57719b9657a45199a79264a9437940cd55b3c7b3fc99615654f124a0 AS dependencies
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --ignore-scripts --no-audit && install -d -m 0700 /session

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2 AS runtime
WORKDIR /app
COPY --from=dependencies /usr/local/bin/node /usr/bin/node
COPY --from=dependencies /app/node_modules ./node_modules
COPY --chown=65532:65532 --from=dependencies /session /var/lib/vidora-auth
COPY src ./src
ENV NODE_ENV=production
USER 65532:65532
CMD ["node", "src/main.mjs"]

FROM runtime AS test
COPY test ./test
RUN ["node", "--test"]

FROM runtime AS production
