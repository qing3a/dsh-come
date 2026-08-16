/**
 * dsh-recruit-workbench —— 浏览器工作台（Client 半）
 *
 * 手写 ModuleLoader 格式 bundle（无构建步骤）：window.__ModuleLoader__.load({id, factory})。
 * 由 clientModules 以 /plugins/recruit-workbench/client.js 提供，`dsh.client` 声明在 package.json。
 *
 * 注册到 conversation.view 视图环：在「对话 / 轨迹」标签旁新增「工作台」标签，
 * 渲染仪表盘 / 候选人 / 职位 / 推荐看板 / 活动五个页面，数据走同源
 * /api/recruit-workbench/*（host 半注册的 Web API，与工具共用同一业务逻辑与审计）。
 *
 * 依赖：react（web-frontend 种子模块）+ ctx.slots（dsh-client-runtime 提供）。
 */
window.__ModuleLoader__.load({
	id: "recruit-workbench",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });

		// ---- 样式（随 bundle 注入一次，卸载由 claimStyles 接管） ----
		var css = [
			".rwb{box-sizing:border-box;height:100%;display:flex;flex-direction:column;padding:16px 20px 24px;overflow:auto;color:var(--dsw-alias-label-primary,#1f2937);font-size:13px;line-height:1.5}",
			".rwb *{box-sizing:border-box}",
			".rwb-head{display:flex;align-items:center;gap:10px;margin-bottom:14px;flex-wrap:wrap}",
			".rwb-title{font-size:17px;font-weight:700;display:flex;align-items:center;gap:8px}",
			".rwb-sub{font-size:12px;color:var(--dsw-alias-label-tertiary,#6b7280)}",
			".rwb-tabs{display:flex;gap:6px;margin-bottom:14px;flex-wrap:wrap}",
			".rwb-tab{padding:7px 16px;border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:8px;background:var(--dsw-alias-bg-base,#fff);cursor:pointer;font-size:13px;color:var(--dsw-alias-label-secondary,#374151)}",
			".rwb-tab:hover{background:var(--dsw-alias-interactive-bg-hover,#f3f4f6)}",
			".rwb-tab.on{background:#2563eb;border-color:#2563eb;color:#fff;font-weight:600}",
			".rwb-err{padding:10px 14px;border-radius:8px;background:#fef2f2;border:1px solid #fecaca;color:#991b1b;margin-bottom:12px;font-size:13px}",
			".rwb-kpis{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:10px;margin-bottom:16px}",
			".rwb-kpi{background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:10px;padding:12px 14px}",
			".rwb-kpi .n{font-size:24px;font-weight:800;font-variant-numeric:tabular-nums}",
			".rwb-kpi .l{font-size:12px;color:var(--dsw-alias-label-tertiary,#6b7280);margin-top:2px}",
			".rwb-cols{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:12px;margin-bottom:16px}",
			".rwb-col{background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:10px;padding:12px}",
			".rwb-col h4{margin:0 0 8px;font-size:13px;display:flex;justify-content:space-between}",
			".rwb-funnel{display:flex;flex-direction:column;gap:6px}",
			".rwb-funnel .row{display:flex;align-items:center;gap:8px;font-size:12px}",
			".rwb-funnel .bar{height:14px;border-radius:4px;background:#dbeafe;min-width:2px}",
			".rwb-funnel .cnt{font-variant-numeric:tabular-nums;color:var(--dsw-alias-label-secondary,#374151)}",
			".rwb-card{background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:10px;padding:12px 14px;margin-bottom:10px;cursor:pointer}",
			".rwb-card:hover{border-color:#93c5fd;box-shadow:0 2px 8px rgba(37,99,235,.08)}",
			".rwb-card .nm{font-weight:600;display:flex;align-items:center;gap:8px;flex-wrap:wrap}",
			".rwb-card .mt{font-size:12px;color:var(--dsw-alias-label-tertiary,#6b7280);margin-top:3px}",
			".rwb-badge{font-size:11px;padding:1px 8px;border-radius:999px;background:#eff6ff;color:#2563eb;white-space:nowrap}",
			".rwb-badge.g{background:#f0fdf4;color:#15803d}.rwb-badge.r{background:#fef2f2;color:#b91c1c}.rwb-badge.y{background:#fffbeb;color:#b45309}",
			".rwb-input,.rwb-select,.rwb-textarea{padding:7px 10px;border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:7px;font-size:13px;background:var(--dsw-alias-bg-base,#fff);color:inherit;width:100%}",
			".rwb-input:focus,.rwb-select:focus,.rwb-textarea:focus{outline:none;border-color:#2563eb}",
			".rwb-btn{padding:7px 14px;border:none;border-radius:7px;background:#2563eb;color:#fff;cursor:pointer;font-size:13px}",
			".rwb-btn:hover{background:#1d4ed8}",
			".rwb-btn.ghost{background:transparent;color:var(--dsw-alias-label-secondary,#374151);border:1px solid var(--dsw-alias-border-l2,#e5e7eb)}",
			".rwb-btn.danger{background:transparent;color:#dc2626;border:1px solid #fecaca}",
			".rwb-btn.small{padding:3px 10px;font-size:12px}",
			".rwb-btn:disabled{opacity:.5;cursor:not-allowed}",
			".rwb-form{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:8px;background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:10px;padding:12px;margin-bottom:14px}",
			".rwb-form .wide{grid-column:1/-1}",
			".rwb-form h4{margin:0;grid-column:1/-1;font-size:13px}",
			".rwb-search{display:flex;gap:8px;margin-bottom:12px}",
			".rwb-kanban{display:grid;grid-template-columns:repeat(7,minmax(150px,1fr));gap:10px;overflow-x:auto;align-items:start}",
			".rwb-lane{background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:10px;padding:10px;min-height:120px}",
			".rwb-lane .tt{font-size:12px;font-weight:700;margin-bottom:8px;display:flex;justify-content:space-between;align-items:center}",
			".rwb-lane .cnt{font-size:11px;font-weight:400;color:var(--dsw-alias-label-tertiary,#6b7280)}",
			".rwb-kcard{background:var(--dsw-alias-interactive-bg-hover,#f8fafc);border:1px solid var(--dsw-alias-border-l2,#e5e7eb);border-radius:8px;padding:8px 10px;margin-bottom:8px;font-size:12px}",
			".rwb-kcard .who{font-weight:600}",
			".rwb-kcard .acts{display:flex;gap:4px;margin-top:6px;flex-wrap:wrap}",
			".rwb-tl{position:relative;padding-left:18px}",
			".rwb-tl:before{content:'';position:absolute;left:5px;top:4px;bottom:4px;width:2px;background:var(--dsw-alias-border-l2,#e5e7eb)}",
			".rwb-tl-item{position:relative;margin-bottom:12px}",
			".rwb-tl-item:before{content:'';position:absolute;left:-16px;top:5px;width:10px;height:10px;border-radius:50%;background:#2563eb;border:2px solid var(--dsw-alias-bg-base,#fff)}",
			".rwb-tl-item .t{font-size:11px;color:var(--dsw-alias-label-tertiary,#6b7280)}",
			".rwb-tl-item .x{font-size:13px;margin-top:2px}",
			".rwb-overlay{position:fixed;inset:0;background:rgba(15,23,42,.4);display:flex;align-items:center;justify-content:center;z-index:300;padding:20px}",
			".rwb-modal{background:var(--dsw-alias-bg-base,#fff);border-radius:12px;max-width:640px;width:100%;max-height:86vh;overflow:auto;padding:18px 20px;box-shadow:0 20px 50px rgba(0,0,0,.25)}",
			".rwb-modal h3{margin:0 0 12px;display:flex;justify-content:space-between;align-items:center}",
			".rwb-fld{display:flex;gap:6px;margin-bottom:6px;font-size:13px}",
			".rwb-fld .k{flex:none;width:88px;color:var(--dsw-alias-label-tertiary,#6b7280)}",
			".rwb-fld .v{word-break:break-word}",
			".rwb-empty{color:var(--dsw-alias-label-tertiary,#9ca3af);text-align:center;padding:24px 0;font-size:13px}",
			".rwb-section{margin-bottom:14px}",
			".rwb-section h4{font-size:14px;margin:0 0 8px}"
		].join("\n");
		var tagId = "recruit-workbench/workbench.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			var tag = document.createElement("style");
			tag.dataset.plugin = "recruit-workbench";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}

		// ---- 依赖与常量 ----
		var React = require("react");
		var useState = React.useState;
		var useEffect = React.useEffect;
		var useCallback = React.useCallback;
		var useMemo = React.useMemo;

		var API = "/api/recruit-workbench";
		var STAGE_LABELS = { recommended: "已推荐", pending_client: "待客户反馈", interviewing: "面试中", offer_sent: "已发Offer", hired: "已入职", rejected: "已拒绝", withdrawn: "已撤回" };
		var STAGE_NEXT = { recommended: "pending_client", pending_client: "interviewing", interviewing: "offer_sent", offer_sent: "hired" };
		var CHAIN = ["recommended", "pending_client", "interviewing", "offer_sent", "hired"];
		var CAND_LABELS = { sourcing: "寻源", contacted: "已联系", interviewing: "面试中", offered: "已发Offer", placed: "已入职", archived: "已归档" };
		var KIND_LABELS = { comm: "沟通", interview: "面试", offer: "Offer", note: "备注", system: "系统" };
		var TARGET_LABELS = { candidate: "候选人", position: "职位", referral: "推荐", company: "公司" };
		var POS_LABELS = { open: "招聘中", paused: "暂停", closed: "关闭" };
		var OFFER_LABELS = { draft: "草稿", sent: "已发", accepted: "已接受", declined: "已拒绝" };

		function el(type, props) {
			var children = Array.prototype.slice.call(arguments, 2);
			return React.createElement.apply(React, [type, props].concat(children));
		}
		function esc(s) { return String(s == null ? "" : s); }
		function fmt(ts) { try { return new Date(ts).toLocaleString(); } catch { return esc(ts); } }

		async function apiState() {
			var r = await fetch(API + "/state");
			return r.json();
		}
		async function apiMutate(op, args) {
			var r = await fetch(API + "/mutate", {
				method: "POST",
				headers: { "content-type": "application/json" },
				body: JSON.stringify({ op: op, args: args || {} })
			});
			return r.json();
		}

		// ---- 小组件 ----
		function Badge(props) {
			var cls = "rwb-badge";
			if (props.tone) cls += " " + props.tone;
			return el("span", { className: cls }, props.children);
		}

		function Empty(props) {
			return el("div", { className: "rwb-empty" }, props.children);
		}

		function Kpi(props) {
			return el("div", { className: "rwb-kpi" },
				el("div", { className: "n" }, props.value),
				el("div", { className: "l" }, props.label));
		}

		function Modal(props) {
			if (!props.open) return null;
			return el("div", { className: "rwb-overlay", onClick: props.onClose },
				el("div", { className: "rwb-modal", onClick: function (e) { e.stopPropagation(); } }, props.children));
		}

		function Field(props) {
			return el("div", { className: "rwb-fld" },
				el("span", { className: "k" }, props.k),
				el("span", { className: "v" }, esc(props.v == null ? "—" : props.v)));
		}

		// ---- 主应用 ----
		function WorkbenchApp() {
			var state = useState(null); var data = state[0]; var setData = state[1];
			var stateE = useState(null); var error = stateE[0]; var setError = stateE[1];
			var stateT = useState("dashboard"); var tab = stateT[0]; var setTab = stateT[1];
			var stateQ = useState(""); var query = stateQ[0]; var setQuery = stateQ[1];
			var stateD = useState(null); var detail = stateD[0]; var setDetail = stateD[1];
			var stateB = useState(false); var busy = stateB[0]; var setBusy = stateB[1];
			var stateR = useState(0); var refresh = stateR[1];

			var load = useCallback(function () {
				apiState().then(function (j) {
					if (j.ok) { setData(j.data); setError(null); }
					else setError(j.error || "加载失败");
				}).catch(function (e) { setError("无法连接工作台 API：" + e.message); });
			}, []);

			useEffect(function () { load(); }, [load, refresh]);

			var mutate = useCallback(function (op, args) {
				setBusy(true); setError(null);
				return apiMutate(op, args).then(function (j) {
					if (j.ok) { setData(j.state); return j.result; }
					setError(j.error || "操作失败"); return null;
				}).catch(function (e) { setError("请求失败：" + e.message); return null; })
					.finally(function () { setBusy(false); });
			}, []);

			var openDetail = useCallback(function (type, id) { setDetail({ type: type, id: id }); }, []);
			var closeDetail = useCallback(function () { setDetail(null); }, []);

			var counts = data ? data.counts : null;
			var q = query.trim().toLowerCase();

			var candList = useMemo(function () {
				if (!data) return [];
				return data.candidates.filter(function (c) {
					if (!q) return true;
					return [c.name, c.title, c.company, c.city].concat(c.tags || []).join(" ").toLowerCase().indexOf(q) >= 0;
				});
			}, [data, q]);

			var posList = useMemo(function () {
				if (!data) return [];
				return data.positions.filter(function (p) {
					if (!q) return true;
					return [p.title, p.client, p.city, p.requirements].join(" ").toLowerCase().indexOf(q) >= 0;
				});
			}, [data, q]);

			var candById = useMemo(function () {
				var m = {};
				(data ? data.candidates : []).forEach(function (c) { m[c.id] = c; });
				return m;
			}, [data]);
			var posById = useMemo(function () {
				var m = {};
				(data ? data.positions : []).forEach(function (p) { m[p.id] = p; });
				return m;
			}, [data]);

			return el("div", { className: "rwb" },
				el("div", { className: "rwb-head" },
					el("span", { className: "rwb-title" }, "🕵️ 猎头工作台"),
					el("span", { className: "rwb-sub" }, "本地优先 · 全流程留痕 · 数据落本机")),
				el("div", { className: "rwb-tabs" },
					el("button", { className: "rwb-tab" + (tab === "dashboard" ? " on" : ""), onClick: function () { setTab("dashboard"); } }, "📊 仪表盘"),
					el("button", { className: "rwb-tab" + (tab === "candidates" ? " on" : ""), onClick: function () { setTab("candidates"); } }, "👤 候选人" + (counts ? " " + counts.candidates : "")),
					el("button", { className: "rwb-tab" + (tab === "positions" ? " on" : ""), onClick: function () { setTab("positions"); } }, "💼 职位" + (counts ? " " + counts.positions : "")),
					el("button", { className: "rwb-tab" + (tab === "referrals" ? " on" : ""), onClick: function () { setTab("referrals"); } }, "📋 推荐看板" + (counts ? " " + counts.referrals : "")),
					el("button", { className: "rwb-tab" + (tab === "activities" ? " on" : ""), onClick: function () { setTab("activities"); } }, "🧾 沟通留痕")),
				error ? el("div", { className: "rwb-err" }, "⚠️ " + error) : null,
				tab === "dashboard" ? DashView({ data: data, candById: candById, posById: posById }) : null,
				tab === "candidates" ? CandidatesView({ data: data, list: candList, query: query, setQuery: setQuery, mutate: mutate, openDetail: openDetail, busy: busy }) : null,
				tab === "positions" ? PositionsView({ data: data, list: posList, query: query, setQuery: setQuery, mutate: mutate, openDetail: openDetail, busy: busy }) : null,
				tab === "referrals" ? ReferralsView({ data: data, candById: candById, posById: posById, mutate: mutate, busy: busy }) : null,
				tab === "activities" ? ActivitiesView({ data: data, mutate: mutate, busy: busy }) : null,
				el(DetailModal, { detail: detail, data: data, candById: candById, posById: posById, onClose: closeDetail, mutate: mutate, busy: busy }));
		}

		// ---- 仪表盘 ----
		function DashView(props) {
			var data = props.data;
			if (!data) return el(Empty, null, "加载中…");
			var counts = data.counts;
			var funnelRows = data.referralsByStage || [];
			var total = funnelRows.reduce(function (s, r) { return s + r.count; }, 0);
			var candRows = data.candidatesByStage || [];
			var recent = data.recentActivities || [];
			return el("div", null,
				el("div", { className: "rwb-kpis" },
					el(Kpi, { value: counts.candidates, label: "候选人" }),
					el(Kpi, { value: counts.positions, label: "职位（open " + counts.openPositions + "）" }),
					el(Kpi, { value: counts.referrals, label: "推荐" }),
					el(Kpi, { value: counts.hired, label: "已入职" }),
					el(Kpi, { value: counts.interviews, label: "面试" }),
					el(Kpi, { value: counts.offers, label: "Offer" })),
				el("div", { className: "rwb-cols" },
					el("div", { className: "rwb-col" },
						el("h4", null, "推荐漏斗", el("span", { className: "rwb-sub" }, "共 " + total + " 条")),
						funnelRows.length ? el("div", { className: "rwb-funnel" }, funnelRows.map(function (r) {
							return el("div", { className: "row", key: r.stage },
								el("span", { style: { width: 84 } }, STAGE_LABELS[r.stage] || r.stage),
								el("div", { className: "bar", style: { width: Math.max(2, total ? (r.count / total) * 120 : 2) } }),
								el("span", { className: "cnt" }, r.count));
						})) : el(Empty, null, "暂无推荐")),
					el("div", { className: "rwb-col" },
						el("h4", null, "候选人阶段"),
						candRows.length ? el("div", { className: "rwb-funnel" }, candRows.map(function (r) {
							return el("div", { className: "row", key: r.stage },
								el("span", { style: { width: 84 } }, CAND_LABELS[r.stage] || r.stage),
								el("span", { className: "cnt" }, r.count));
						})) : el(Empty, null, "暂无候选人"))),
				el("div", { className: "rwb-col", style: { marginBottom: 0 } },
					el("h4", null, "最近活动"),
					recent.length ? el("div", { className: "rwb-tl" }, recent.map(function (a) {
						return el("div", { className: "rwb-tl-item", key: a.id },
							el("div", { className: "t" }, fmt(a.createdAt) + " · " + (KIND_LABELS[a.kind] || a.kind) + " · " + (TARGET_LABELS[a.targetType] || a.targetType)),
							el("div", { className: "x" }, esc(a.text)));
					})) : el(Empty, null, "暂无活动记录")));
		}

		// ---- 候选人 ----
		function CandidatesView(props) {
			var data = props.data; if (!data) return null;
			var stateF = useState(false); var formOpen = stateF[0]; var setFormOpen = stateF[1];
			var f = {};
			["name", "title", "company", "city", "phone", "email", "resume", "salaryExpect", "tags", "notes"].forEach(function (k) { f[k] = useState(""); });
			var stateSt = useState("sourcing"); var stage = stateSt[0]; var setStage = stateSt[1];

			function bind(k) {
				var s = f[k];
				return { value: s[0], onChange: function (e) { s[1](e.target.value); } };
			}
			function submit() {
				var args = { name: f.name[0].trim(), stage: stage };
				if (!args.name) { alert("姓名必填"); return; }
				["title", "company", "city", "phone", "email", "resume", "salaryExpect", "tags", "notes"].forEach(function (k) {
					if (f[k][0].trim()) args[k] = f[k][0].trim();
				});
				if (args.tags) args.tags = args.tags.split(/[,，]/).map(function (s) { return s.trim(); }).filter(Boolean);
				props.mutate("register_candidate", args).then(function () {
					Object.keys(f).forEach(function (k) { f[k][1](""); });
					setStage("sourcing");
					setFormOpen(false);
				});
			}

			return el("div", null,
				el("div", { className: "rwb-search" },
					el("input", { className: "rwb-input", placeholder: "搜索候选人（姓名/职位/公司/标签）…", value: props.query, onChange: function (e) { props.setQuery(e.target.value); } }),
					el("button", { className: "rwb-btn" + (formOpen ? " ghost" : ""), onClick: function () { setFormOpen(!formOpen); } }, formOpen ? "收起表单" : "➕ 新增候选人")),
				formOpen ? el("div", { className: "rwb-form" },
					el("h4", null, "新增候选人（隐私：只记必要事实，敏感字段默认保密）"),
					el("input", Object.assign({ className: "rwb-input", placeholder: "姓名 *" }, bind("name"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "当前职位" }, bind("title"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "现公司" }, bind("company"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "城市" }, bind("city"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "电话（保密）" }, bind("phone"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "邮箱（保密）" }, bind("email"))),
					el("select", { className: "rwb-select", value: stage, onChange: function (e) { setStage(e.target.value); } }, Object.keys(CAND_LABELS).map(function (k) {
						return el("option", { key: k, value: k }, CAND_LABELS[k]);
					})),
					el("input", Object.assign({ className: "rwb-input", placeholder: "薪资期望（保密）" }, bind("salaryExpect"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "标签，逗号分隔" }, bind("tags"))),
					el("textarea", Object.assign({ className: "rwb-textarea wide", rows: 2, placeholder: "简历要点（必要事实，不编造）" }, bind("resume"))),
					el("textarea", Object.assign({ className: "rwb-textarea wide", rows: 2, placeholder: "备注" }, bind("notes"))),
					el("button", { className: "rwb-btn wide", disabled: props.busy, onClick: submit }, "保存候选人"))
					: null,
				props.list.length ? props.list.map(function (c) {
					return el("div", { className: "rwb-card", key: c.id, onClick: function () { props.openDetail("candidate", c.id); } },
						el("div", { className: "nm" }, esc(c.name), c.title ? el(Badge, null, esc(c.title)) : null),
						el("div", { className: "mt" }, [c.company, c.city].filter(Boolean).join(" · ") || "未填写公司"),
						el("div", { className: "mt", style: { display: "flex", gap: 6, flexWrap: "wrap", marginTop: 6 } },
							el(Badge, { tone: c.stage === "placed" ? "g" : c.stage === "archived" ? "y" : "" }, CAND_LABELS[c.stage] || c.stage),
							(c.tags || []).slice(0, 4).map(function (t) { return el(Badge, { key: t }, esc(t)); })));
				}) : el(Empty, null, "暂无候选人"));
		}

		// ---- 职位 ----
		function PositionsView(props) {
			var data = props.data; if (!data) return null;
			var stateF = useState(false); var formOpen = stateF[0]; var setFormOpen = stateF[1];
			var f = {};
			["client", "title", "city", "quantity", "requirements", "niceToHave", "salaryRange", "notes"].forEach(function (k) { f[k] = useState(""); });
			var stateSt = useState("open"); var status = stateSt[0]; var setStatus = stateSt[1];

			function bind(k) {
				var s = f[k];
				return { value: s[0], onChange: function (e) { s[1](e.target.value); } };
			}
			function submit() {
				var args = { client: f.client[0], title: f.title[0], status: status };
				["city", "quantity", "requirements", "niceToHave", "salaryRange", "notes"].forEach(function (k) {
					if (f[k][0]) args[k] = k === "quantity" ? Number(f[k][0]) : f[k][0];
				});
				if (!args.client || !args.title) { alert("客户与职位名必填"); return; }
				props.mutate("register_position", args).then(function () {
					Object.keys(f).forEach(function (k) { f[k][1](""); });
					setStatus("open");
					setFormOpen(false);
				});
			}

			return el("div", null,
				el("div", { className: "rwb-search" },
					el("input", { className: "rwb-input", placeholder: "搜索职位（名称/客户/要求）…", value: props.query, onChange: function (e) { props.setQuery(e.target.value); } }),
					el("button", { className: "rwb-btn" + (formOpen ? " ghost" : ""), onClick: function () { setFormOpen(!formOpen); } }, formOpen ? "收起表单" : "➕ 新增职位")),
				formOpen ? el("div", { className: "rwb-form" },
					el("h4", null, "新增职位需求（薪资区间默认保密）"),
					el("input", Object.assign({ className: "rwb-input", placeholder: "客户公司 *" }, bind("client"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "职位名称 *" }, bind("title"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "工作地点" }, bind("city"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "招聘人数", type: "number" }, bind("quantity"))),
					el("input", Object.assign({ className: "rwb-input", placeholder: "薪资区间（保密）" }, bind("salaryRange"))),
					el("select", { className: "rwb-select", value: status, onChange: function (e) { setStatus(e.target.value); } }, Object.keys(POS_LABELS).map(function (k) {
						return el("option", { key: k, value: k }, POS_LABELS[k]);
					})),
					el("textarea", Object.assign({ className: "rwb-textarea wide", rows: 2, placeholder: "硬性要求" }, bind("requirements"))),
					el("textarea", Object.assign({ className: "rwb-textarea wide", rows: 2, placeholder: "软性要求/加分项" }, bind("niceToHave"))),
					el("textarea", Object.assign({ className: "rwb-textarea wide", rows: 2, placeholder: "备注" }, bind("notes"))),
					el("button", { className: "rwb-btn wide", disabled: props.busy, onClick: submit }, "保存职位"))
					: null,
				props.list.length ? props.list.map(function (p) {
					return el("div", { className: "rwb-card", key: p.id, onClick: function () { props.openDetail("position", p.id); } },
						el("div", { className: "nm" }, esc(p.title), el(Badge, { tone: p.status === "open" ? "" : p.status === "closed" ? "r" : "y" }, POS_LABELS[p.status] || p.status)),
						el("div", { className: "mt" }, [p.client, p.city].filter(Boolean).join(" · ")));
				}) : el(Empty, null, "暂无职位"));
		}

		// ---- 推荐看板 ----
		function ReferralsView(props) {
			var data = props.data; if (!data) return null;
			var stateF = useState(false); var formOpen = stateF[0]; var setFormOpen = stateF[1];
			var stateC = useState(""); var candId = stateC[0]; var setCandId = stateC[1];
			var stateP = useState(""); var posId = stateP[0]; var setPosId = stateP[1];
			var stateN = useState(""); var note = stateN[0]; var setNote = stateN[1];

			function submit() {
				if (!candId || !posId) { alert("请选择候选人与职位"); return; }
				props.mutate("create_referral", { candidateId: candId, positionId: posId, note: note }).then(function () {
					setCandId(""); setPosId(""); setNote(""); setFormOpen(false);
				});
			}

			function stageAction(r, stage, label) {
				return el("button", { className: "rwb-btn small" + (stage === "rejected" || stage === "withdrawn" ? " danger" : ""), disabled: props.busy, onClick: function (e) { e.stopPropagation(); props.mutate("update_referral_stage", { referralId: r.id, stage: stage }); } }, label);
			}

			var lanes = CHAIN.concat(["rejected", "withdrawn"]);
			return el("div", null,
				el("div", { className: "rwb-search" },
					el("button", { className: "rwb-btn" + (formOpen ? " ghost" : ""), onClick: function () { setFormOpen(!formOpen); } }, formOpen ? "收起表单" : "➕ 发起推荐")),
				formOpen ? el("div", { className: "rwb-form" },
					el("h4", null, "发起推荐（进入 7 态流水线）"),
					el("select", { className: "rwb-select", value: candId, onChange: function (e) { setCandId(e.target.value); } },
						el("option", { value: "" }, "选择候选人…"),
						data.candidates.map(function (c) { return el("option", { key: c.id, value: c.id }, c.name + (c.title ? " · " + c.title : "")); })),
					el("select", { className: "rwb-select", value: posId, onChange: function (e) { setPosId(e.target.value); } },
						el("option", { value: "" }, "选择职位…"),
						data.positions.filter(function (p) { return p.status !== "closed"; }).map(function (p) { return el("option", { key: p.id, value: p.id }, p.title + " · " + p.client); })),
					el("input", Object.assign({ className: "rwb-input", placeholder: "推荐说明" }, { value: note, onChange: function (e) { setNote(e.target.value); } })),
					el("button", { className: "rwb-btn wide", disabled: props.busy, onClick: submit }, "发起推荐"))
					: null,
				el("div", { className: "rwb-kanban" }, lanes.map(function (stage) {
					var rows = data.referrals.filter(function (r) { return r.stage === stage; });
					return el("div", { className: "rwb-lane", key: stage },
						el("div", { className: "tt" }, STAGE_LABELS[stage] || stage, el("span", { className: "cnt" }, rows.length)),
						rows.length ? rows.map(function (r) {
							var c = props.candById[r.candidateId];
							var p = props.posById[r.positionId];
							var next = STAGE_NEXT[stage];
							return el("div", { className: "rwb-kcard", key: r.id },
								el("div", { className: "who" }, esc(c ? c.name : r.candidateId)),
								el("div", { style: { marginTop: 2 } }, esc(p ? p.title + " · " + p.client : r.positionId)),
								r.note ? el("div", { className: "mt" }, esc(r.note)) : null,
								el("div", { className: "acts" },
									next ? stageAction(r, next, "→ " + (STAGE_LABELS[next] || next)) : null,
									!next && stage === "hired" ? null : stageAction(r, "rejected", "拒绝"),
									!next && stage === "withdrawn" ? null : stageAction(r, "withdrawn", "撤回")));
						}) : el(Empty, null, "空"));
				})));
		}

		// ---- 沟通留痕 ----
		function ActivitiesView(props) {
			var data = props.data; if (!data) return null;
			var stateK = useState("comm"); var kind = stateK[0]; var setKind = stateK[1];
			var stateT = useState("candidate"); var targetType = stateT[0]; var setTargetType = stateT[1];
			var stateI = useState(""); var targetId = stateI[0]; var setTargetId = stateI[1];
			var stateX = useState(""); var text = stateX[0]; var setText = stateX[1];

			var targets = [];
			if (targetType === "candidate") targets = data.candidates;
			else if (targetType === "position") targets = data.positions;
			else if (targetType === "company") targets = data.companies;
			else targets = data.referrals;

			function submit() {
				if (!targetId || !text.trim()) { alert("请选择对象并填写内容"); return; }
				props.mutate("add_activity", { kind: kind, targetType: targetType, targetId: targetId, text: text }).then(function () {
					setText(""); setTargetId("");
				});
			}
			function labelOf(t) {
				if (targetType === "candidate") return t.name + (t.title ? " · " + t.title : "");
				if (targetType === "position") return t.title + " · " + t.client;
				if (targetType === "company") return t.name;
				return t.candidateId || t.id;
			}

			var list = (data.activities || []).slice().reverse();
			return el("div", null,
				el("div", { className: "rwb-form" },
					el("h4", null, "记录一条沟通/进展（事实留痕，不编造）"),
					el("select", { className: "rwb-select", value: kind, onChange: function (e) { setKind(e.target.value); } }, Object.keys(KIND_LABELS).map(function (k) {
						return el("option", { key: k, value: k }, KIND_LABELS[k]);
					})),
					el("select", { className: "rwb-select", value: targetType, onChange: function (e) { setTargetType(e.target.value); setTargetId(""); } }, Object.keys(TARGET_LABELS).map(function (k) {
						return el("option", { key: k, value: k }, TARGET_LABELS[k]);
					})),
					el("select", { className: "rwb-select", value: targetId, onChange: function (e) { setTargetId(e.target.value); } },
						el("option", { value: "" }, "选择对象…"),
						targets.map(function (t) { return el("option", { key: t.id, value: t.id }, labelOf(t)); })),
					el("textarea", { className: "rwb-textarea wide", rows: 2, placeholder: "内容（涉敏请注明保密）", value: text, onChange: function (e) { setText(e.target.value); } }),
					el("button", { className: "rwb-btn wide", disabled: props.busy, onClick: submit }, "保存留痕")),
				list.length ? el("div", { className: "rwb-tl" }, list.map(function (a) {
					return el("div", { className: "rwb-tl-item", key: a.id },
						el("div", { className: "t" }, fmt(a.createdAt) + " · " + (KIND_LABELS[a.kind] || a.kind) + " · " + (TARGET_LABELS[a.targetType] || a.targetType) + " · " + esc(a.targetId)),
						el("div", { className: "x" }, esc(a.text)));
				})) : el(Empty, null, "暂无留痕"));
		}

		// ---- 详情弹窗 ----
		function DetailModal(props) {
			var d = props.detail;
			if (!d) return null;
			var data = props.data; if (!data) return null;
			var onClose = props.onClose;

			var body = null;
			if (d.type === "candidate") {
				var c = data.candidates.filter(function (x) { return x.id === d.id; })[0];
				if (!c) return el(Modal, { open: true, onClose: onClose }, el("div", null, "候选人不存在"));
				var refs = data.referrals.filter(function (r) { return r.candidateId === c.id; });
				var acts = data.activities.filter(function (a) { return a.targetType === "candidate" && a.targetId === c.id; });
				var offs = data.offers.filter(function (o) { return o.candidateId === c.id; });
				var ints = data.interviews.filter(function (i) { return i.candidateId === c.id; });
				body = el("div", null,
					el("div", { className: "rwb-section" },
						el(Field, { k: "姓名", v: c.name }),
						el(Field, { k: "职位", v: c.title }),
						el(Field, { k: "现公司", v: c.company }),
						el(Field, { k: "城市", v: c.city }),
						el(Field, { k: "电话（保密）", v: c.phone }),
						el(Field, { k: "邮箱（保密）", v: c.email }),
						el(Field, { k: "薪资期望（保密）", v: c.salaryExpect }),
						el(Field, { k: "阶段", v: CAND_LABELS[c.stage] || c.stage }),
						el(Field, { k: "标签", v: (c.tags || []).join("、") }),
						el(Field, { k: "简历要点", v: c.resume }),
						el(Field, { k: "备注", v: c.notes })),
					refs.length ? el("div", { className: "rwb-section" },
						el("h4", null, "推荐记录"),
						refs.map(function (r) {
							var p = props.posById[r.positionId];
							return el("div", { className: "rwb-kcard", key: r.id },
								el("div", null, esc(p ? p.title + " · " + p.client : r.positionId), " ", el(Badge, null, STAGE_LABELS[r.stage] || r.stage)));
						})) : null,
					ints.length ? el("div", { className: "rwb-section" },
						el("h4", null, "面试"),
						ints.map(function (i) { return el("div", { className: "rwb-kcard", key: i.id }, el("div", null, esc(i.round) + " · " + esc(i.when) + " · " + esc(i.mode)), i.note ? el("div", { className: "mt" }, esc(i.note)) : null); })) : null,
					offs.length ? el("div", { className: "rwb-section" },
						el("h4", null, "Offer"),
						offs.map(function (o) { return el("div", { className: "rwb-kcard", key: o.id }, el("div", null, OFFER_LABELS[o.status] || o.status, o.package ? " · " + esc(o.package) : "")); })) : null,
					acts.length ? el("div", { className: "rwb-section" },
						el("h4", null, "沟通留痕"),
						acts.map(function (a) { return el("div", { className: "rwb-tl-item", key: a.id, style: { marginBottom: 8 } }, el("div", { className: "t" }, fmt(a.createdAt)), el("div", { className: "x" }, esc(a.text))); })) : null,
					el("div", { style: { display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 12 } },
						el("button", { className: "rwb-btn ghost", onClick: onClose }, "关闭"),
						el("button", { className: "rwb-btn danger", onClick: function () {
							if (confirm("确认删除候选人「" + c.name + "」？此操作会写入审计。")) {
								props.mutate("delete_entity", { type: "candidate", id: c.id, confirm: true }).then(onClose);
							}
						} }, "删除候选人")));
			} else if (d.type === "position") {
				var p2 = data.positions.filter(function (x) { return x.id === d.id; })[0];
				if (!p2) return el(Modal, { open: true, onClose: onClose }, el("div", null, "职位不存在"));
				var refs2 = data.referrals.filter(function (r) { return r.positionId === p2.id; });
				body = el("div", null,
					el("div", { className: "rwb-section" },
						el(Field, { k: "客户", v: p2.client }),
						el(Field, { k: "职位", v: p2.title }),
						el(Field, { k: "地点", v: p2.city }),
						el(Field, { k: "人数", v: p2.quantity }),
						el(Field, { k: "状态", v: POS_LABELS[p2.status] || p2.status }),
						el(Field, { k: "薪资（保密）", v: p2.salaryRange }),
						el(Field, { k: "硬性要求", v: p2.requirements }),
						el(Field, { k: "软性/加分", v: p2.niceToHave }),
						el(Field, { k: "备注", v: p2.notes })),
					refs2.length ? el("div", { className: "rwb-section" },
						el("h4", null, "推荐记录"),
						refs2.map(function (r) {
							var c = props.candById[r.candidateId];
							return el("div", { className: "rwb-kcard", key: r.id },
								el("div", null, esc(c ? c.name : r.candidateId), " ", el(Badge, null, STAGE_LABELS[r.stage] || r.stage)));
						})) : null,
					el("div", { style: { display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 12 } },
						el("button", { className: "rwb-btn ghost", onClick: onClose }, "关闭"),
						el("button", { className: "rwb-btn danger", onClick: function () {
							if (confirm("确认删除职位「" + p2.title + "」？")) {
								props.mutate("delete_entity", { type: "position", id: p2.id, confirm: true }).then(onClose);
							}
						} }, "删除职位")));
			} else {
				body = el("div", null, "未知详情类型：" + esc(d.type));
			}

			return el(Modal, { open: true, onClose: onClose },
				el("h3", null,
					el("span", null, d.type === "candidate" ? "👤 候选人详情" : "💼 职位详情"),
					el("button", { className: "rwb-btn ghost small", onClick: onClose }, "✕")),
				body);
		}

		// ---- 插件主体：注册「工作台」视图标签 ----
		function apply(ctx) {
			ctx.slots.inject("conversation.view", function () {
				return ctx.slots.register({
					name: "conversation.view",
					id: "recruit-workbench",
					order: 20,
					label: "工作台"
				}, WorkbenchView);
			});
		}

		function WorkbenchView() {
			return el(WorkbenchApp, null);
		}

		exports.apply = apply;
		exports.inject = ["slots"];
		return module.exports;
	}
});
