## axum脚手架

### 参考

1. [serve-with-hyper](https://github.com/tokio-rs/axum/blob/main/examples/serve-with-hyper/src/main.rs#L81)


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
``