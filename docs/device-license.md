# 设备许可证与交付

发布构建启动前验证离线许可证。许可证由开发机上的 Ed25519 私钥签发，应用只内置公钥；许可证载荷绑定 `meetingbox-rk`、设备硬件指纹和到期时间。私钥不应进入仓库、安装包或客户设备。

## 获取设备指纹

在目标设备上运行：

```bash
sudo /opt/meetingbox/scripts/device-fingerprint.sh
```

如果设备尚未安装应用，也可以从源码目录运行同名脚本。指纹组合板卡序列号、CPU 序列号和系统 machine-id，输出类似 `sha256:...`。

## 签发许可证

仅在开发机上执行，私钥默认位于 `$HOME/.config/meetingbox-license/ed25519-private.pem`，权限应为 `600`：

```bash
frontend/src-tauri/scripts/sign-license.sh \
  --device-id 'sha256:...' \
  --issued-at '2026-08-19T00:00:00Z' \
  --expires-at '2027-08-19T00:00:00Z' \
  --output ./license.json
```

长期授权可以省略 `--expires-at`，但建议销售设备使用明确到期时间，便于撤销和换机管理。

## 安装交付

许可证安装到 `/etc/meetingbox/license.json`，应用包安装到系统目录：

```bash
sudo frontend/src-tauri/scripts/prepare-device-release.sh ./会议助手.deb ./license.json
```

交付前删除源码、`.git`、`target/`、包管理缓存、私钥和调试日志。当前发布配置不再信任上游项目的自动更新地址；后续如需更新，必须配置你自己的签名更新源和公钥。

## 开发调试

仅 debug 构建支持显式环境变量绕过许可证：

```bash
MEETINGBOX_ALLOW_UNLICENSED_DEV=1 pnpm tauri dev
```

release 构建不读取该绕过变量。拥有设备 root 权限的人仍可能逆向二进制，因此许可证应与系统权限、只读交付镜像和签名更新一起使用。
