FROM node:24.22.3-trixie-slim@sha256:af02d57b57719b9657a45199a79264a9437940cd55b3c7b3fc99615654f124a0 AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts --no-audit
COPY index.html vite.config.mjs ./
COPY src ./src
RUN npm run build

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2 AS runtime
WORKDIR /app
COPY --from=build /usr/local/bin/node /usr/bin/node
COPY --from=build /app/dist ./dist
COPY server ./server
ENV NODE_ENV=production
USER 65532:65532
CMD ["node", "server/main.mjs"]

FROM runtime AS test
COPY test ./test
RUN ["node", "--test"]

FROM runtime AS production
