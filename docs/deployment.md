# 部署指南

> Gasket-RS 生产环境部署说明

---

## 部署架构图

```mermaid
flowchart TB
    subgraph Users["用户访问层"]
        TG[Telegram]
        DC[Discord]
        Web[WebSocket]
    end
    
    subgraph Server["服务器层"]
        GW[Gasket Gateway]
        NG[Nginx/Traefik]
    end
    
    subgraph Data["数据层"]
        DB[(SQLite)]
        FS[Memory 文件]
        Vault[Vault 密钥]
    end
    
    subgraph External["外部服务"]
        LLM[LLM API]
        Search[搜索 API]
    end
    
    Users --> NG
    NG --> GW
    GW --> DB
    GW --> FS
    GW --> Vault
    GW --> LLM
    GW --> Search
```

---

## 1. 二进制部署

### 1.1 构建 Release 版本

```bash
# 克隆仓库
git clone https://github.com/YeHeng/gasket-rs.git
cd gasket-rs

# 构建（启用所有渠道）
cargo build --release

# 或仅启用指定渠道
cargo build --release --no-default-features \
    --features "telegram,discord,websocket"

# 二进制位置
./target/release/gasket
```

### 1.2 系统服务部署 (systemd)

创建服务文件 `/etc/systemd/system/gasket.service`：

```ini
[Unit]
Description=Gasket AI Agent Gateway
After=network.target

[Service]
Type=simple
User=gasket
Group=gasket
WorkingDirectory=/opt/gasket
Environment="RUST_LOG=info"
Environment="GASKET_CONFIG=/opt/gasket/config.yaml"
ExecStart=/opt/gasket/gasket gateway
Restart=always
RestartSec=10

# 安全限制
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/gasket/data
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true

[Install]
WantedBy=multi-user.target
```

部署脚本：

```bash
# 创建用户
sudo useradd -r -s /bin/false gasket

# 部署文件
sudo mkdir -p /opt/gasket
cp target/release/gasket /opt/gasket/
cp config.yaml /opt/gasket/
sudo chown -R gasket:gasket /opt/gasket

# 启动服务
sudo systemctl daemon-reload
sudo systemctl enable gasket
sudo systemctl start gasket

# 查看日志
sudo journalctl -u gasket -f
```

---

## 2. Docker 部署

### 2.1 Dockerfile

```dockerfile
# 构建阶段
FROM rust:1.75-bookworm as builder

WORKDIR /app
COPY . .

RUN cargo build --release --features "telegram,discord,websocket"

# 运行阶段
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# 创建非 root 用户
RUN useradd -m -u 1000 gasket

WORKDIR /app

# 复制二进制文件
COPY --from=builder /app/target/release/gasket /usr/local/bin/

# 创建工作目录
RUN mkdir -p /data && chown gasket:gasket /data

USER gasket

# 数据卷
VOLUME ["/data"]

EXPOSE 18790

ENTRYPOINT ["gasket"]
CMD ["status"]
```

### 2.2 docker-compose.yml

```yaml
version: '3.8'

services:
  gasket:
    build: .
    container_name: gasket
    restart: unless-stopped
    
    environment:
      - RUST_LOG=info
      - GASKET_CONFIG=/data/config.yaml
    
    volumes:
      - ./data:/data
      - ./config.yaml:/data/config.yaml:ro
    
    ports:
      - "18790:18790"
    
    # 资源限制
    deploy:
      resources:
        limits:
          cpus: '2'
          memory: 2G
        reservations:
          cpus: '0.5'
          memory: 512M
```

部署：

```bash
docker-compose up -d
docker-compose logs -f
```

---

## 3. Kubernetes 部署

### 3.1 ConfigMap

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: gasket-config
data:
  config.yaml: |
    providers:
      openrouter:
        api_key: ${OPENROUTER_API_KEY}
    
    agents:
      defaults:
        model: openrouter/anthropic/claude-4.5-sonnet
    
    gateway:
      session_timeout: 3600
```

### 3.2 Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: gasket-secrets
type: Opaque
stringData:
  OPENROUTER_API_KEY: "sk-or-v1-xxx"
  TELEGRAM_TOKEN: "xxx"
  VAULT_PASSWORD: "your-vault-password"
```

### 3.3 Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: gasket
spec:
  replicas: 2
  selector:
    matchLabels:
      app: gasket
  template:
    metadata:
      labels:
        app: gasket
    spec:
      containers:
      - name: gasket
        image: your-registry/gasket:latest
        ports:
        - containerPort: 18790
        env:
        - name: RUST_LOG
          value: "info"
        - name: GASKET_CONFIG
          value: "/config/config.yaml"
        - name: OPENROUTER_API_KEY
          valueFrom:
            secretKeyRef:
              name: gasket-secrets
              key: OPENROUTER_API_KEY
        volumeMounts:
        - name: config
          mountPath: /config
          readOnly: true
        - name: data
          mountPath: /data
        resources:
          requests:
            memory: "512Mi"
            cpu: "500m"
          limits:
            memory: "2Gi"
            cpu: "2000m"
        livenessProbe:
          httpGet:
            path: /health
            port: 18790
          initialDelaySeconds: 30
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /ready
            port: 18790
          initialDelaySeconds: 5
          periodSeconds: 5
      volumes:
      - name: config
        configMap:
          name: gasket-config
      - name: data
        persistentVolumeClaim:
          claimName: gasket-data
```

### 3.4 Service

```yaml
apiVersion: v1
kind: Service
metadata:
  name: gasket
spec:
  selector:
    app: gasket
  ports:
  - port: 80
    targetPort: 18790
  type: ClusterIP
```

### 3.5 HPA (水平自动扩缩容)

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: gasket-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: gasket
  minReplicas: 2
  maxReplicas: 10
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: 70
  - type: Resource
    resource:
      name: memory
      target:
        type: Utilization
        averageUtilization: 80
```

部署：

```bash
kubectl apply -f k8s/
kubectl get pods -l app=gasket
kubectl logs -f deployment/gasket
```

---

## 4. 反向代理配置

### 4.1 Nginx

```nginx
upstream gasket {
    server 127.0.0.1:8080;
    keepalive 32;
}

server {
    listen 80;
    server_name your-domain.com;
    
    # WebSocket 支持
    location /ws {
        proxy_pass http://gasket;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_read_timeout 86400;
    }
    
    # API 代理
    location / {
        proxy_pass http://gasket;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

### 4.2 Traefik

```yaml
http:
  routers:
    gasket:
      rule: "Host(`your-domain.com`)"
      service: gasket
      tls:
        certResolver: letsencrypt
    
    gasket-ws:
      rule: "Host(`your-domain.com`) && PathPrefix(`/ws`)"
      service: gasket
      tls:
        certResolver: letsencrypt
  
  services:
    gasket:
      loadBalancer:
        servers:
        - url: "http://gasket:8080"
```

---

## 5. 监控与日志

### 5.1 日志配置

```yaml
# config.yaml
logging:
  level: info
  format: json  # 生产环境使用 JSON 格式便于解析
```

### 5.2 日志收集 (Fluent Bit)

```ini
[INPUT]
    Name              systemd
    Tag               gasket
    Systemd_Filter    _SYSTEMD_UNIT=gasket.service

[OUTPUT]
    Name              loki
    Match             gasket
    Host              loki.example.com
    Port              3100
```

### 5.3 日志收集 (Fluent Bit)

```ini
[INPUT]
    Name              systemd
    Tag               gasket
    Systemd_Filter    _SYSTEMD_UNIT=gasket.service

[OUTPUT]
    Name              loki
    Match             gasket
    Host              loki.example.com
    Port              3100
```

---

## 6. 备份与恢复

### 6.1 备份策略

```bash
#!/bin/bash
# backup.sh - 每日备份

BACKUP_DIR="/backup/gasket/$(date +%Y%m%d)"
mkdir -p "$BACKUP_DIR"

# 备份 SQLite 数据库
sqlite3 ~/.gasket/gasket.db ".backup '$BACKUP_DIR/gasket.db'"

# 备份记忆文件
tar czf "$BACKUP_DIR/memory.tar.gz" -C ~/.gasket memory/

# 备份 Vault
tar czf "$BACKUP_DIR/vault.tar.gz" -C ~/.gasket vault/

# 备份配置
cp ~/.gasket/config.yaml "$BACKUP_DIR/"

# 保留最近 7 天
find /backup/gasket -type d -mtime +7 -exec rm -rf {} +
```

### 6.2 恢复

```bash
# 停止服务
sudo systemctl stop gasket

# 恢复数据
cp /backup/gasket/20240101/gasket.db ~/.gasket/
tar xzf /backup/gasket/20240101/memory.tar.gz -C ~/.gasket
tar xzf /backup/gasket/20240101/vault.tar.gz -C ~/.gasket

# 启动服务
sudo systemctl start gasket
```

---

## 7. 安全配置

### 7.1 文件权限

```bash
# 配置目录权限
chmod 700 ~/.gasket
chmod 600 ~/.gasket/config.yaml
chmod 600 ~/.gasket/gasket.db
chmod 700 ~/.gasket/vault
```

### 7.2 防火墙规则

```bash
# 仅允许必要端口
sudo ufw allow 22/tcp      # SSH
sudo ufw allow 80/tcp      # HTTP
sudo ufw allow 443/tcp     # HTTPS
sudo ufw enable
```

### 7.3 Vault 加密

生产环境务必启用 Vault 加密：

```bash
# 设置环境变量
export GASKET_MASTER_PASSWORD="your-strong-password"

# 或使用 systemd 的 LoadCredential
# /etc/systemd/system/gasket.service.d/override.conf
[Service]
LoadCredential=vault_password:/etc/gasket/vault_password
```

---

## 8. 性能调优

### 8.1 SQLite 优化

```sql
-- 在连接时执行
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = 10000;
PRAGMA temp_store = memory;
```

### 8.2 系统限制

```ini
# /etc/systemd/system/gasket.service.d/limits.conf
[Service]
LimitNOFILE=65536
LimitNPROC=4096
```

### 8.3 内核参数

```bash
# /etc/sysctl.d/99-gasket.conf
net.core.somaxconn = 65535
net.ipv4.tcp_max_syn_backlog = 65535
vm.max_map_count = 262144
```
