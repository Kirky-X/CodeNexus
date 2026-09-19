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
