# Build from the repo root so the pnpm workspace resolves:
#   docker build -f site/Dockerfile -t stakedevtool-site .
FROM node:22-alpine AS build
RUN npm install -g pnpm@10
WORKDIR /repo

# Workspace manifests first so dependency install is cached across code changes.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY site/package.json site/
COPY ui/package.json ui/
COPY web/package.json web/
RUN pnpm install --frozen-lockfile --filter site

COPY site/ site/
RUN pnpm --filter site build

FROM node:22-alpine
ENV NODE_ENV=production
ENV PORT=3000
COPY --from=build /repo/site/.output /app
EXPOSE 3000
USER node
CMD ["node", "/app/server/index.mjs"]
