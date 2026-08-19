// dsh-health-plugin — 为 `dsh web` 暴露 /api/health 健康端点。
//
// 用途：dsh-come 看门狗用 health_ok() 优先探测 /api/health，缺失则降级到首页 /。
// 配此插件后，看门狗能区分「进程在但 web 无响应」与「整服务挂了」，探活更干净。
//
// ⚠️ 这是参考实现，不是开箱即用的自动加载文件：
// dsh 是 Cordis 架构（everything is a plugin），插件必须经你的 dsh 组合
// （cordis.yml / preset）注册；dsh 不会自动扫描 ~/.dsh/plugins 下的裸 .js。
// 启用方式（任选其一）：
//   1) 包成 @deepseek-ai/dsh-health 并在你的 cordis.yml 组合里 enable；
//   2) 在你的 preset 里以 inline 插件形式挂载本 apply(ctx)。
// 若当前 `dsh web` 组合未激活 webServer 服务，apply 会安全跳过（不报错）。

export function apply(ctx) {
  let webServer
  try {
    webServer = ctx.webServer
  } catch {
    // 当前组合未提供 webServer 服务（非 web 场景）→ 不注册，直接退出
    return
  }
  webServer.register({
    kind: 'exact',
    path: '/api/health',
    handler(_req, res) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ ok: true, ts: Date.now() }))
    },
  })
}
