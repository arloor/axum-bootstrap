## axum脚手架

### 参考

1. [axum serve-with-hyper](https://github.com/tokio-rs/axum/blob/main/examples/serve-with-hyper/src/main.rs#L81)
2. [axum hyper graceful shutdown](https://github.com/hyperium/hyper-util/blob/master/examples/server_graceful.rs)
3. [axum anyhow-error-response](https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs)
4. [axum error-handling](https://github.com/tokio-rs/axum/blob/main/examples/error-handling/src/main.rs)

### TLS自签名证书

```bash
openssl req -x509 -newkey rsa:4096 -sha256 -nodes -keyout privkey.pem -out cert.pem -days 3650 -subj "/C=cn/ST=hl/L=sd/O=op/OU=as/CN=example.com"
```

### MySQL容器

```bash
#rm -rf /var/lib/mysql
docker stop mysql
mkdir -p /var/lib/mysql /etc/mysql/conf.d
cat > /etc/mysql/conf.d/ssl.cnf <<EOF
[mysqld]
default-time_zone = '+8:00'
require_secure_transport=ON
EOF
docker run -d --rm  --name mysql \
--network host \
-v /var/lib/mysql:/var/lib/mysql \
-v /etc/mysql/conf.d:/etc/mysql/conf.d \
-e MYSQL_DATABASE=test \
-e MYSQL_ROOT_PASSWORD=xxxxxx \
docker.io/library/mysql:9.1 
```
