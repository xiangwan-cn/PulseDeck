# PulseDeck 卡片配置指南

编辑 `~/.config/pulsedeck/config.toml`，复制一个 `[[cards]]` 配置块并修改 `id`、
`title`、`order`、`renderer` 和数据源即可。内置渲染器包括 `value`、
`progress`、`status`、`text`、`action`；`list` 和 `composite` 只由插件卡片产出，
普通数据源卡片选择它们会显示空白（见文末「配置限制与严格字段」）。数据源包括
`builtin`、`file`、`command`、`http` 和 `static_value`。

当前配置接口固定为 schema v2，文件第一项必须是：

```toml
schema_version = 2
```

PulseDeck 不迁移或忽略旧接口：版本不符、未知字段、旧别名和未知枚举值都会使整份配置
加载失败；热重载失败时继续保留上一次成功加载的运行配置。修改 schema 时应同时修改
实际使用的 `~/.config/pulsedeck/config.toml`，而不是在程序中增加兼容分支。
启动时被拒绝的配置也不会被默认配置或可选卡片自动写回覆盖。

例如，启用已内置但默认不显示的系统负载：

```toml
[[cards]]
id = "load"
title = "系统负载"
page = "monitor"
order = 100
renderer = "value"
refresh_interval = 30
enabled = true
icon = "utilities-system-monitor-symbolic"

[cards.source]
type = "builtin"
metric = "load_average"
```

可用的隐藏原生指标：

- `load_average`：1/5/15 分钟系统负载
- `swap`：交换空间占用
- `process_count`：进程数量
- `cpu_temperature`：CPU/SoC 温度
- `filesystem`：根文件系统占用
- `network_traffic`：上下行实时速率

任意 Shell、Python、Go 或独立 Rust 程序都可通过 `command` 数据源成为卡片，
因此自定义业务逻辑也不需要重新编译 PulseDeck。配置文件保存后会被重新读取；新增或
删除卡片后重新打开应用即可重建页面。

完整可复制配置见 `config/config.example.toml`；`config.example.json` 使用相同的
当前 schema。每个 `id` 应在自己的类型中保持唯一，卡片引用的 `page` 必须存在。

## 通用卡片字段

```toml
[[cards]]
id = "unique-id"              # 必填，稳定且唯一
title = "标题"                 # 必填
page = "monitor"              # 必填，对应 [[pages]].id
order = 10                    # 数值越小越靠前
renderer = "value"            # value/progress/status/text/list/composite/action
refresh_interval = 30         # 秒
enabled = true
icon = "computer-symbolic"    # 可选，Freedesktop 图标名
description = "说明"           # 可选
cache_ttl_seconds = 300        # 可选
click_action = "action-id"     # 可选，点击整张卡片时执行对应 [[actions]]
# schedule = "daily@08:00,20:00"
```

`refresh_interval` 控制普通轮询间隔。设置 `schedule` 后，应用按每日固定时间生成独立
缓存周期。失败任务会进行有上限的退避，避免持续快速重试。

调度器保留这里的原始间隔，再根据统一运行模式计算实际间隔。普通模式使用原值；
空闲模式按卡片类别降频；检测到外接电源在线时只对明确允许的低开销指标升频；后台暂停非必要
刷新。模式变化会立即重算下一次截止时间，手动刷新始终立即执行。临近任务会在小窗口
内合并唤醒，但不会提前执行固定时间计划。

可用 `[cards.runtime]` 覆盖自动分类：

```toml
[cards.runtime]
class = "command"
# 精确取值：auto、system-realtime、network-rate、network-status、battery-thermal、
# command、http、file、static；省略时为 auto，并按数据源自动归类。
idle_behavior = "throttle"     # throttle 或 pause
idle_multiplier = 8.0
external_realtime = false      # 高开销命令/HTTP 默认不要打开
realtime_multiplier = 0.75
minimum_interval_seconds = 5
```

`click_action` 只引用已有 `[[actions]].id`，不会在卡片中重复保存命令。是否显示
二次确认由对应 action 的 `confirm` 控制；设置 `visible = false` 可只保留点击入口，
不在操作页渲染重复按钮。卡片内的刷新按钮仍只刷新数据，不触发 action。使用
`renderer = "action"` 时，其“执行”按钮触发同一个 `click_action`；执行期间显示 spinner
并禁用按钮，结果返回后恢复。未配置有效 `click_action` 时按钮保持禁用。

`Number` 和 `Percentage` 主数值由所有相关渲染器共享同一格式化规则：整数百分比不保留
无意义的 `.0`，温度和中文量词直接紧跟数值，`W`、`GiB` 等拉丁单位保留空格，非有限
数值显示为 `—`。网络卡以 IP 为主值，连接状态和连接名称放在下方；无法取得 IP 时回退
显示“已连接/受限/未连接”等状态。IP 使用普通前景色；电池容量达到 80% 时主值显示
绿色，降到 20% 时显示红色，中间电量使用普通前景色；
功耗卡始终以瓦数为主值，预计剩余或充满时间放在下方，避免主值语义随充放电变化。

需要确认时可使用 `confirm_title` 和 `confirm_detail` 自定义确认页内容。省略后会使用
action 名称作为标题、action 描述作为说明，因此确认页仍会指出即将执行的操作。

### 卡片点击动作完整实例

把「点击卡片」和「执行命令」组合起来的可复制配置：状态卡展示服务运行状态，点击卡片
经确认后切换启停；action 设置 `visible = false` 时不占用操作页空间：

```toml
[[cards]]
id = "service-status"
title = "服务状态"
page = "monitor"
order = 90
renderer = "status"
refresh_interval = 5
icon = "system-run-symbolic"
description = "点击卡片切换服务"
click_action = "toggle-service"

[cards.source]
type = "command"
program = "sh"
args = ["-c", "if pgrep -f '[m]yservice' >/dev/null 2>&1; then echo '● 运行中'; else echo '○ 已停止'; fi"]
timeout_seconds = 5

[[actions]]
id = "toggle-service"
name = "切换服务"
description = "根据当前状态启动或停止服务"
icon = "system-run-symbolic"
page = "actions"
visible = false
timeout = 30
confirm = true
confirm_title = "切换服务？"
confirm_detail = "运行中则停止，未运行则启动。"
command = ["sh", "-c", "if pgrep -f '[m]yservice' >/dev/null 2>&1; then pkill -f '[m]yservice'; echo 已停止; else setsid nohup myservice >>\"$HOME/.local/state/myservice.log\" 2>&1 </dev/null & sleep 1; echo 已启动; fi"]
```

要点：

- 命令源直接执行 `program`，不经 shell；轻量场景可用 `sh -c` 包一层，复杂逻辑推荐
  写成独立脚本作为 `program`（见「命令」）。
- `pgrep -f` 会匹配完整命令行，用 `[m]yservice` 字符类写法可避免匹配到执行检测的
  `sh -c` 自身，否则状态会恒为“运行中”、停止时还会误杀自己。
- 后台启动的进程务必 `</dev/null` 并把 stdout/stderr 重定向到日志文件，否则会占住
action 的输出管道，导致结果对话框迟迟不返回。
- `renderer = "action"` 的卡片会显示一个“执行”按钮，触发同一个 `click_action`；
  执行期间按钮禁用并显示 spinner，未配置有效 `click_action` 时按钮保持禁用。

## 页面

页面是卡片与操作的分组容器，顶栏按 `order` 排序展示。默认内置 monitor/actions/settings
三个页面（配置缺省时自动兜底），新增页面只需追加 `[[pages]]` 块，卡片用 `page` 引用：

```toml
[[pages]]
id = "tools"
title = "工具"
icon = "utilities-system-monitor-symbolic"  # 可选，顶栏图标
order = 25

[[cards]]
id = "my-card"
title = "我的卡片"
page = "tools"      # 引用上面的页面 id
order = 10
renderer = "value"
refresh_interval = 30
icon = "computer-symbolic"
```

页面 `kind` 字段可挂载编译期插件页面（如 `kind = "scrcpy-forge"`），插件页面由
`[pages.plugin]` 提供专属配置，不参与通用卡片系统，见「可选 ScrcpyForge 页面」。

## 数据源示例

### 内置指标

内置指标不启动外部进程，适合系统监控：

```toml
[cards.source]
type = "builtin"
metric = "filesystem"
```

全部名称：`cpu`、`memory`、`uptime`、`battery_capacity`、
`battery_temperature`、`power`、`network`、`load_average`、`swap`、
`process_count`、`cpu_temperature`、`filesystem`、`network_traffic`。
没有电池或温度传感器的设备会显示不可用，不需要按机型修改配置。

### 文件

```toml
[cards.source]
type = "file"
path = "/proc/sys/kernel/hostname"

[cards.source.options]
first_line_only = true
```

文件源适合 procfs、sysfs 或普通文本。路径属于运行设备的本地配置；公开示例不要写入
个人主目录。文件卡首次读取后由目录文件监视器触发更新，不再周期唤醒；手动刷新仍然
可用。

### 命令

```toml
[cards.source]
type = "command"
program = "uname"
args = ["-r"]
timeout_seconds = 5
max_output_bytes = 4096

[cards.source.options]
reverse_lines = false
max_subtitle_lines = 3
```

`program` 和 `args` 直接传给子进程，不经过 shell。需要管道、重定向或变量展开时，
应在自己的本地脚本中实现，并将脚本作为 `program`；不要把未经信任的内容拼入命令。
命令失败时，卡片只显示 stderr 最后一个非空行（例如 Python traceback 最后的
`RuntimeError` 消息），完整 stderr 保留在 tooltip 中，因此离线错误不会撑高卡片。

### HTTP

```toml
[cards.source]
type = "http"
url = "https://example.invalid/api/status"
method = "GET"
timeout_seconds = 10
max_output_bytes = 65536
headers = { Accept = "application/json" }

[cards.source.parser]
type = "json_path"
path = "data.status"
```

POST/PUT 等请求可用 `body` 提供请求体。HTTP 方法支持 `GET`、`POST`、`PUT`、`DELETE`
和 `PATCH`。真实服务地址、认证请求头
和 token 只应写入被 Git 忽略的本地配置，不要提交到公开仓库。
HTTP、command 和 file 数据源实例会长期复用；正则解析器只编译一次。

### 静态值

```toml
[cards.source]
type = "static_value"

[cards.source.options]
value = "本地仪表盘"
```

静态值适合说明、分组提示或暂不需要轮询的数据。
它只在首次加载或配置重建时求值，不进入周期调度。

## HTTP 解析器

- `json_path`：用点号访问对象或数组，如 `data.items.0.value`；数值文本加
  `as_percentage = true` 可转为百分比。
- `regex`：使用 `pattern` 和可选的 `capture`（捕获组索引，默认 1）提取文本。
- `number`：使用 `multiplier`、`divisor`、`decimal_places` 和 `suffix` 转为数值。
- `first_line`：只保留第一行，并可追加 `suffix`。

正则提取实例（从 `Load: 0.5` 中提取数字）：

```toml
[cards.source.parser]
type = "regex"
pattern = "Load: ([0-9.]+)"
capture = 1
```

数值解析示例：

```toml
[cards.source.parser]
type = "number"
divisor = 1000
decimal_places = 1
suffix = "°C"
```

## 操作按钮

操作仅在用户点击时运行，不是周期卡片：

```toml
[[actions]]
id = "system-summary"
name = "查看系统信息"
description = "显示当前系统内核和架构"
icon = "utilities-system-monitor-symbolic"
page = "actions"
command = ["uname", "-a"]
timeout = 5
confirm = false
max_output_bytes = 4096
```

会修改系统状态的操作应设置 `confirm = true`。PulseDeck 不会自动提权；若本地操作
需要管理员权限，应由用户明确配置 `pkexec` 等授权工具。

`[[actions]]` 可用字段：`id`、`name` 必填；`description`、`icon` 可选；`page` 指定
渲染页面；`visible = false` 时不渲染按钮，仅作为卡片 `click_action` 的隐藏入口；
`command` 为 `["程序", "参数", ...]`；`timeout` 默认 10 秒；`confirm = true` 时点击
先弹确认框，可用 `confirm_title`/`confirm_detail` 自定义文案（省略则用 name/description）；
`max_output_bytes` 可单独覆盖全局输出上限。

## 卡片尺寸

默认普通模式使用三列、紧凑模式使用六列，两种模式都会根据页面当前可见高度计算
三行卡片高度，因此分别可让 3×3 或 6×3 张卡片刚好填满可见区域。`card_height`
是最小高度：窗口较高时卡片自动增高，窗口过小时则保持下限并允许页面滚动。
较长内容会自动切换为较小字号，不再把整行卡片撑高：

```toml
[ui]
card_columns = 3
card_height = 133 # 自适应高度的下限
fixed_card_size = true
# card_width = 180 # 省略时自动等分可用宽度
```

单张卡片可以在 `[cards.display]` 中覆盖全局设置：

```toml
[cards.display]
card_width = 220
card_height = 160
fixed_size = false # false 表示内容较多时允许卡片继续增高
minimum_change = 5.0   # 主数值变化低于该阈值时不重绘（省电、防闪烁）
columns_after = 5      # value 渲染器 + 多行文本时：超过 5 行切换多列
columns = 2            # 多列模式列数，默认 2
```

`columns_after`/`columns` 只对 `renderer = "value"` 且命令输出多行文本生效——这是
普通数据源唯一能做出“列表”效果的方式（`list` 渲染器本身仅插件可用）。

单卡片尺寸覆盖仍是下限；例如 `card_height = 160` 会阻止该卡片缩到 160 像素以下，
但不会阻止页面在空间充足时把三行统一增高。

## 普通卡片的状态与颜色

普通非插件卡片可以把采集结果映射为命名视觉状态。状态规则按配置顺序求值，**首条匹配
规则生效**；没有规则匹配时保留现有渲染器、主题颜色和数据源状态行为。规则只在已有的
数据更新中求值，不增加轮询、动画帧或后台工作。

以下温度卡展示数值范围、状态文案、图标、多色背景和颜色过渡的完整组合：

```toml
[cards.display.transition]
duration_ms = 220             # 0 表示关闭；最大按 5000 ms 处理
easing = "ease-out"           # linear/ease/ease-in/ease-out/ease-in-out

[[cards.display.states]]
name = "comfortable"
max = 39.9                    # min/max 均为闭区间

[cards.display.states.colors]
accent = "#33d17a"
value = "#33d17a"

[[cards.display.states]]
name = "warm"
min = 40.0
max = 44.9

[cards.display.states.colors]
accent = "#e5a50a"
value = "#e5a50a"
background = ["#e5a50a", "#ffbe6f"]

[[cards.display.states]]
name = "hot"
min = 45.0
label = "温度过高"             # 状态期间替换主显示文案
icon = "dialog-warning-symbolic"

[cards.display.states.colors]
accent = "#e01b24"
value = "#e01b24"
background = ["#e01b24", "#9141ac"]
background_opacity = 0.16
```

每条 `[[cards.display.states]]` 支持以下匹配字段：

- `source_state`：数据源生命周期，取值为 `normal`、`loading`、`unavailable`、`error`
  或 `cached`。它可以让加载、失败、不可用和缓存状态也拥有专属文案与配色。
- `min` / `max`：主值的闭区间边界，适用于 `Number`、`Percentage`，也适用于内容可直接
  解析为数字或百分比的文本。
- `equals` / `contains` / `regex`：匹配主文本；`ignore_case = true` 同时作用于三者。
  正则只在加载配置时编译一次；无效正则不会匹配。
- `status_level`：匹配 `status` 值的 `good`、`normal`、`warning`、`critical`、`error`
  或 `unknown` 语义级别。

同一规则中填写的条件使用“并且”关系；例如 `source_state = "normal"` 加
`contains = "运行"` 要求两者同时成立。完全不写匹配字段的规则是兜底规则，应放在列表
末尾。`name` 用于标识状态，同一张卡片内应保持唯一。`label` 与 `icon` 是可选展示覆盖；
离开该状态后会自动恢复数据值和卡片原图标。

基础颜色放在 `[cards.display.colors]`，状态颜色放在对应的
`[cards.display.states.colors]`。状态层只覆盖自己声明的区域，其余区域继续继承基础颜色
或主题默认值：

```toml
[cards.display.colors]
accent = "#3584e4"            # 左侧强调边
value = "#f6f5f4"             # 主值及加载/错误提示
title = "#ffffff"
icon = "#99c1f1"
subtitle = "#deddda"          # 顶部静态说明
footer = "#c0bfbc"            # 底部动态说明/缓存标记
progress = "#62a0ea"          # progress 渲染器的填充块
background = ["#3584e4", "#9141ac", "#2190a0"]
background_opacity = 0.12      # 0.0–1.0，默认 0.12
```

颜色只接受 `#RGB`、`#RGBA`、`#RRGGBB` 或 `#RRGGBBAA`，无效值会被忽略，避免把配置
内容注入 GTK CSS。一个 `background` 颜色生成纯色淡化背景，两个或更多颜色生成从左上
到右下的渐变；建议保持默认低透明度，以延续 PulseDeck 克制、信息优先的视觉风格。
没有任何颜色配置时，CPU、内存、电池等原有自动强调色和 status/progress 语义色完全
不变。自定义 `value` 后会有意覆盖渲染器的默认语义色；需要按状态变化时应把它放到各
状态的 `colors` 中。

状态切换通过 GTK CSS 完成，只对颜色、背景色和边框色做过渡，不启动逐帧动画。
`reload_on_change = true` 时，修改规则、颜色、文案或过渡会清除上次显示结果并立即请求
一次刷新，因此无需重启；新增、删除卡片或改变页面层级仍需重新打开应用。

这些字段只属于普通卡片。带 `kind` 的 PetCard 等插件卡片继续使用各插件自己的视觉与
状态配置，不会被通用状态规则改写。

页面切换栏右上角的网格按钮可在默认列数和六列紧凑布局间切换。紧凑模式仍保留标题、
主要数值、顶部说明和底部状态文字，只隐藏占宽明显的图标与刷新按钮。选择通过
`${XDG_STATE_HOME:-$HOME/.local/state}/pulsedeck/compact-grid` 原子保存，下次启动
及配置热重载后继续使用上次模式。

设置页最多使用 `[ui].card_columns` 指定的列数，并限制为三列；竖屏宽度不足时自动
降为两列或一列，避免设置控件的最小宽度撑大整个页面和顶栏。

## 固定时间更新

命令、HTTP 和外部插件卡片都可以声明每日更新时间。应用会为每个时间点生成
独立缓存周期，同一周期最多执行一次；如果应用在时间点之后才启动，会补取当天
最近一期，而不会要求应用恰好在该分钟运行：

```toml
schedule = "daily@08:05,12:05,16:05,20:05"
cache_ttl_seconds = 14400
```

该机制不依赖卡片 ID、标题或脚本内容，任意定时联网或命令卡片都可直接复用。
`schedule` 目前仅支持 `daily@HH:MM,HH:MM` 这一种格式（多时间点用逗号分隔），
不支持 cron 表达式。
持久缓存前有进程内缓存；相同结果至少间隔一段时间才重写磁盘，新的计划周期或变化的
结果仍会原子写入，避免每次成功刷新都产生写放大。

## 运行与省电

全局 `[runtime]` 的推荐默认值见 `config.example.toml`。应用前台且
`keep_screen_on = true` 时同时抑制熄屏和休眠；进入空闲模式只改变应用内显示与刷新，
不会释放常亮。应用后台立即释放抑制并暂停非必要调度。真实点击、触摸、滚动、键盘、
拖动、页面切换、手动刷新和插件控制会重置空闲时间；自动刷新、网络请求、动画及状态
文件变化不会。

手动刷新单张卡片后，刷新按钮会暂时禁用，结果返回后自动恢复；旧值在刷新期间保持可见。
加载、错误和不可用状态使用与渲染器无关的静态提示，因此列表或组合卡片不会继续显示
上一次成功内容。确认对话框打开期间持有最长五分钟的交互保护，用户响应后立即释放。

网络连接状态通过 NetworkManager D-Bus 读取并由 GIO 网络事件触发，不再为每次刷新
启动多个 `nmcli`。CPU/内存/Swap 等相邻采集会复用短时 `/proc` 快照。配置中的
`minimum_change` 先比较稳定的主数值，动态 subtitle/tooltip 不会绕过阈值并造成 GTK
重复重绘。电池 sysfs 中带符号的 `power_now`、`power_avg` 和 `current_now` 会按绝对
功率读取，充放电方向仍以电池 `status` 为准。

## 可选 ScrcpyForge 页面

编译 `scrcpy-forge` feature 后会自动加入缺失的默认页面，不需要再手动让 AI 配置；
已有同 ID 页面保持不变。示例文件仅用于覆盖服务地址、尺寸或端点等默认值。

该页面不属于通用卡片系统，默认不会编译。使用
`cargo build --release --features scrcpy-forge` 启用，配置见
`src/plugins/scrcpy_forge/config.example.toml`。页面不可见时停止预览和健康请求；
重新进入后自动恢复。空闲模式仅更新设备元数据，不下载预览 PNG；服务端支持 ETag
时未变化图片不传输，客户端内容哈希未变化时不重建纹理。每台设备拥有对应的预览卡和
脚本卡。温度达到 warm 或更高等级时，预览和健康检查间隔自动延长；hot/throttled
状态下 PetCard 动画冻结在当前帧并移除动画定时器。

## 可选 PetCard

PetCard 默认不编译。使用 `cargo build --release --features pet-card` 启用；编译进该
feature 后会自动加入缺失的 `codex-pet` 卡片，emoji 回退无需额外配置。自定义帧配置、
Codex hook、四格/六格/全屏尺寸、离线回落和完成提示音见
[`docs/PET_CARD.md`](../docs/PET_CARD.md)。

## 配置限制与严格字段

schema v2 使用严格字段解析。以下限制会直接影响配置是否可被加载：

- `renderer = "list"` / `"composite"`：普通数据源（builtin/command/file/http/static_value）
  无法产出列表或组合值，选择后卡片显示空白；仅插件卡片内部使用。
- 数据源只有 `builtin`、`file`、`command`、`http`、`static_value`；旧的 `static`
  别名会被拒绝。
- 不存在 `source.shell` 字段；需要 shell 语义时明确设置 `program = "sh"`、
  `args = ["-c", "..."]`，或把脚本写成本地文件作为 `program`。
- HTTP 解析器只有 `json_path`、`regex`、`number`、`first_line`；不存在
  `parser.steps`、`template` 或 `divide` 接口。
- `[cards.runtime].class` 只接受本文列出的精确值，不接受 `system`、`network`、
  `battery`、`thermal` 等旧别名。
- `schedule`：仅支持 `daily@HH:MM,HH:MM` 一种格式，没有 cron。
- 页面与卡片的 `id` 各自类型内唯一；卡片引用的 `page` 必须存在。
