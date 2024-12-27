## axum脚手架


### MySQL容器

```bash
#rm -rf /var/lib/mysql
docker stop mysql
mkdir -p /var/lib/mysql /etc/mysql/conf.d
cat > /etc/mysql/conf.d/ssl.cnf <<EOF
[mysqld]
default-time_zone = '+8:00'
ssl_ca=/etc/mysql/ssl/ca.cer
ssl_cert=/etc/mysql/ssl/arloor.dev.cer
ssl_key=/etc/mysql/ssl/arloor.dev.key
require_secure_transport=ON
EOF
docker run -d --rm  --name mysql \
--network host \
-v /var/lib/mysql:/var/lib/mysql \
-v /etc/mysql/conf.d:/etc/mysql/conf.d \
-v /root/.acme.sh/arloor.dev:/etc/mysql/ssl \
-e MYSQL_DATABASE=test \
-e MYSQL_ROOT_PASSWORD=xxxxxx \
docker.io/library/mysql:9.1 
``