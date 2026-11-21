# Match sandbox: linux/amd64, 1 CPU, 512 MB, read-only root, map at /arena.
FROM oven/bun:1.4.4-slim@sha256:1846adf22496bd711036aae8be147e57bdebbaea2e5264e3e0b5af1db187d9a4
WORKDIR /bot
COPY package.json bot.ts map.ts mechfight.ts ./
ENTRYPOINT ["bun", "/bot/bot.ts"]
