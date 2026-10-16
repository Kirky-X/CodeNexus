# CodeNexus 用户可见消息（zh）。
# 与 en/messages.ftl 的键齐性由 src/i18n/catalog.rs 的 test_key_parity 守卫；
# 内嵌完整性由 embedded_locales_cover_locales_dir 守卫。

# daemon：影响通知（src/daemon/impact_observer.rs）
impact-db-open-failed = 影响告警：无法打开图库，跳过本批次通知
impact-notice = 变更影响告警
impact-webhook-client-failed = 影响告警 webhook：客户端构建失败
impact-webhook-non-2xx = 影响告警 webhook 返回非 2xx
impact-webhook-send-failed = 影响告警 webhook 发送失败

# daemon：增量索引（src/daemon/index_observer.rs）
index-incremental-triggered = 触发增量索引
index-incremental-failed = 增量索引失败，继续监视

# graph-viewer server（axum）错误响应
project-param-required = 需要提供 project 参数
project-not-found = 项目 '{ $name }' 未找到
node-not-found = 节点 '{ $name }' 未找到

# daemon：事件循环生命周期（src/daemon/daemon.rs）
daemon-signals-registered = 信号处理器已注册
daemon-started = 守护模式已启动
daemon-stop-signal-received = 收到停止信号，守护模式退出
daemon-debounce-batch = 处理防抖事件批次
daemon-watch-error = 文件监视器错误
daemon-channel-disconnected = 事件通道已断开，守护模式退出
daemon-shutdown-phase-1 = 停机阶段 1/3：停止接收新文件事件
daemon-shutdown-phase-2 = 停机阶段 2/3：排空防抖事件队列
daemon-shutdown-phase-3 = 停机阶段 3/3：关闭图数据库连接
daemon-shutdown-complete = 分阶段停机完成

# viewer：鉴权/启动日志（src/viewer/server.rs）
graph-host-forbidden = 本服务仅允许本机回环访问
graph-token-missing = 缺少或错误的 x-graph-token 头
graph-server-started = 图数据服务已启动: http://127.0.0.1:{ $port }（请求需带 x-graph-token 头，见 stdout）
graph-server-auth-token = graph-server 鉴权 token: { $auth_token }
graph-project-discovered = 发现项目: { $name } -> { $path }
graph-asset-bad-path = 非法路径
graph-asset-not-found = 资源不存在

# viewer：图查询诊断日志（src/viewer/graph.rs）
graph-node-query-info = 已查询 { $types } 个节点类型，获取 { $nodes } 个节点
graph-edge-scan-info = 边扫描：{ $rows } 行，命中 { $edges } 条边，共 { $total } 个节点

# viewer：内嵌 graph-viewer（src/viewer/assets.rs）
graph-viewer-dist-missing = graph-viewer 前端资源未构建：请在 graph-viewer/ 执行 npm run build 后重新编译本二进制。
graph-viewer-fallback-title = CodeNexus graph-viewer
graph-viewer-fallback-heading = CodeNexus graph-viewer

# logging：inklog 级别预设（src/logging.rs）
logging-invalid_target_level = 忽略非法的 CODENEXUS_LOG_TARGETS 条目 '{ $entry }'（应为 target=level，如 hyper=warn）
logging-invalid_sample_entry = 忽略非法的 CODENEXUS_LOG_SAMPLE 条目 '{ $entry }'（应为 level=N 或 prefix=N[@level]，如 debug=10 或 sqlx=100@warn）
logging-invalid_rate_limit_entry = 忽略非法的 CODENEXUS_LOG_RATE_LIMIT 条目 '{ $entry }'（应为 prefix=rate，如 lbug=50）

# embed：模型完整性摘要校验（src/embed/config.rs）
embed-invalid_model_sha256 = 忽略非法的 CODENEXUS_EMBED_MODEL_SHA256 值 '{ $value }'（应为 64 位十六进制摘要；完整性校验保持关闭）

# embed：本地模型包预检（src/embed/client.rs）
embed-model-not-found = 未在 { $path } 找到本地嵌入模型。请将 arctic-embed-xs 模型放在该路径（或设置 { $env } 环境变量指定自定义位置）。
embed-model-config-missing = 未在 { $path } 找到模型 config.json。
embed-model-weights-missing = 未在 { $path } 找到模型权重（应为 model.safetensors；vecboost Candle 引擎不支持 pytorch_model.bin）。
embed-runtime-build-failed = 构建 tokio 运行时失败：{ $reason }
embed-runtime-thread-panic = vecboost 运行时线程 panic

# cli：用法错误（src/main.rs）
cli-no-subcommand = 未提供子命令（运行 `codenexus --help` 查看命令列表）
