#!/bin/bash
# ─────────────────────────────────────────────────────────────
# rew 官网一键部署脚本
# 用法：bash scripts/deploy-website.sh
# 前提：本地已配置好 SSH 密钥登录服务器
# ─────────────────────────────────────────────────────────────

set -e  # 任何命令失败立即退出

SERVER="root@21.214.197.124"
SSH_PORT=36000
SSH_KEY="$HOME/.ssh/rew_server_key"
SSH_OPTS="-p $SSH_PORT -i $SSH_KEY -o StrictHostKeyChecking=no"
REMOTE_DIR="/var/www/rew"
LOCAL_DIR="$(cd "$(dirname "$0")/../website" && pwd)"

echo "▶ 开始部署 rew 官网..."
echo "  本地目录：$LOCAL_DIR"
echo "  目标服务器：$SERVER:$REMOTE_DIR"
echo ""

# ── 1. 上传网站文件 ──────────────────────────────────────────
echo "📦 [1/3] 上传文件..."
ssh $SSH_OPTS "$SERVER" "mkdir -p $REMOTE_DIR"
rsync -avz --delete \
  -e "ssh $SSH_OPTS" \
  --exclude='.DS_Store' \
  --exclude='*.dmg' \
  "$LOCAL_DIR/" "$SERVER:$REMOTE_DIR/"
echo "✓ 文件上传完成"
echo ""

# ── 2. 清空旧配置，写入新 nginx 配置 ─────────────────────────
echo "⚙  [2/3] 配置 nginx..."
ssh $SSH_OPTS "$SERVER" bash <<'ENDSSH'
set -e

# 清空 conf.d 下所有旧站点配置
rm -f /etc/nginx/conf.d/*.conf
echo "  已清空 /etc/nginx/conf.d/ 下的旧配置"

# 写入 rew 站点配置
cat > /etc/nginx/conf.d/rew.conf << 'EOF'
# 太湖智能网关已处理 HTTPS，回源是 HTTP
# 不需要 HTTP→HTTPS 重定向，否则会造成循环
server {
    listen 80;
    server_name rew-ai.woa.com;

    root /var/www/rew;
    index index.html;
    charset utf-8;

    # 页面路由：直接访问 why / features 等无后缀 URL 时自动加 .html
    location / {
        try_files $uri $uri/ $uri.html =404;
    }

    # 开启 gzip
    gzip on;
    gzip_vary on;
    gzip_min_length 1024;
    gzip_types text/css application/javascript image/svg+xml;

    # 静态资源缓存
    location ~* \.(css|js|png|svg|ico|woff2|webp)$ {
        expires 7d;
        add_header Cache-Control "public, immutable";
    }

    # 禁止访问隐藏文件
    location ~ /\. { deny all; }
}
EOF

echo "  已写入 /etc/nginx/conf.d/rew.conf"

# 语法检查
nginx -t && echo "  nginx 配置语法 OK"
ENDSSH
echo "✓ nginx 配置完成"
echo ""

# ── 3. 重载 nginx ─────────────────────────────────────────────
echo "🔄 [3/3] 重载 nginx..."
ssh $SSH_OPTS "$SERVER" "nginx -s reload"
echo "✓ nginx 已重载"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🎉 部署成功！"
echo "   访问地址：https://rew-ai.woa.com"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
