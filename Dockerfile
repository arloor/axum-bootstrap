FROM alpine as builder
COPY . .
RUN apk add --update --no-cache gcc clang-dev musl-dev curl; \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; \
    export PATH="$HOME/.cargo/bin:$PATH"; \
    cargo build --release --target x86_64-unknown-linux-musl; \
    ls ./target/x86_64-unknown-linux-musl/release/axum-bootstrap

FROM alpine as final
RUN apk add --no-cache tzdata; \
    cp /usr/share/zoneinfo/Asia/Shanghai /etc/localtime; \
    echo "Asia/Shanghai" > /etc/timezone; \
    apk del tzdata
COPY --from=builder ./target/x86_64-unknown-linux-musl/release/axum-bootstrap /axum-bootstrap
ENTRYPOINT ["/axum-bootstrap"]