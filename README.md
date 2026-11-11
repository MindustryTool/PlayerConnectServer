sudo docker pull ghcr.io/mindustrytool/player-connect-server-rust:latest

sudo docker run -d \
  --name player-connect-server-rust \
  --restart unless-stopped \
  -p 7426:7426/udp \
  -p 7429:7429/tcp \
  -e PLAYER_CONNECT_PORT=7426 \
  -e PLAYER_CONNECT_HTTP_PORT=7429 \
  ghcr.io/mindustrytool/player-connect-server-rust:latest
