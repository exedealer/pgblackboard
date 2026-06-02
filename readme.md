# pgBlackboard

Postgres web interface for SQL geeks

- Write SQL in professional code editor
- Run multi-query scripts
- Stream results in real-time
- Edit query results inline
- Visualize PostGIS geometries
- Lightweight

![screenshot](https://raw.githubusercontent.com/exedealer/pgblackboard/refs/heads/main/screenshot.png)

# Docker

[Docker Hub repo](https://hub.docker.com/r/exedealer/pgblackboard)

![Docker Image Size (tag)](https://img.shields.io/docker/image-size/exedealer/pgblackboard/v3?style=for-the-badge)

```sh
docker run -it --rm -p 7890:7890 exedealer/pgblackboard pgbb 'postgres://HOST:5432'
```

```yaml
services:
  pgbb:
    image: exedealer/pgblackboard
    ports: ['7890:7890']
    command: [pgbb, 'postgres://HOST:5432']
```

# Roadmap

- [COPY ... FROM STDIN support](https://github.com/exedealer/pgblackboard/issues/54)
- Geometry drawing

# Sponsorship

If you find this project useful, consider supporting its development!
Your contributions help maintain and improve the project.

[![PayPal](https://img.shields.io/badge/paypal-donate-blue?logo=paypal&logoColor=white&style=for-the-badge)](https://paypal.me/exedealer)
[![Ko-fi](https://img.shields.io/badge/ko--fi-donate-blue?logo=ko-fi&logoColor=white&style=for-the-badge
)](https://ko-fi.com/exedealer)
