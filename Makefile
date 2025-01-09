sql:
	cargo sqlx prepare

docker:
	podman build -f Dockerfile . -t docker.io/arloor/axum-demo  --network host
	# docker build -f Dockerfile . -t docker.io/arloor/axum-demo  --network host --build-arg=http_proxy="http://localhost:3128" --build-arg=https_proxy="http://localhost:3128"