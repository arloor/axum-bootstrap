sql:
	cargo sqlx prepare

docker:
	podman build -f Dockerfile . -t docker.io/arloor/axum-demo  --network host