sql:
	cargo sqlx prepare

docker:
	podman build -f Dockerfile . -t docker.io/arloor/axum-demo  --network host
	# docker build -f Dockerfile . -t docker.io/arloor/axum-demo  --network host --build-arg=HTTP_PROXY="http://localhost:7890" --build-arg=HTTPS_PROXY="http://localhost:7890"