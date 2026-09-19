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

# graph-viewer server：鉴权/启动日志（graph-viewer/server/src/main.rs）
graph-host-forbidden = 本服务仅允许本机回环访问
graph-token-missing = 缺少或错误的 x-graph-token 头
graph-server-started = 图数据服务已启动: http://127.0.0.1:9800（请求需带 x-graph-token 头，见 stdout）
graph-project-discovered = 发现项目: { $name } -> { $path }
