# dc — 音频下载器

只下载音频（不下载视频画面），自动选择当前账号可获取的最高音质，写入完整元数据与封面，支持分片并行与断点续传。纯 Rust 实现，**不依赖 FFmpeg / OpenSSL 等外部程序或系统库**，Windows / macOS / Linux / FreeBSD 均可直接运行。

## 功能

| 功能 | 说明 |
| --- | --- |
| 只要音频 | 仅请求 DASH 音频流；若只有带画面的 MP4，也会在本地剥离视频轨，只保留音频 |
| 最高音质（未登录） | 自动在 64K / 132K / 192K AAC 中选最高，匿名请求带 `try_look` 以获取试看档位 |
| 更高音质（登录） | 登录后可获取 **Hi-Res 无损 FLAC**（`.flac`）与 **杜比全景声 E-AC-3**（`.m4a`），通常需要大会员 |
| 元数据 | 标题、艺术家（UP 主）、专辑（合集名 / 视频名）、专辑艺术家、曲目号/总数、日期、分区（流派）、简介、来源链接、BVID、音质，内嵌封面 |
| 文件名 | 可自定义模板；非法字符替换为全角同形字符（`AC/DC` → `AC／DC`），处理 Windows 保留名、首尾点号空格、长度上限；同批次重名自动追加 BV 号 |
| 分片并行 | 单文件多连接按 Range 分片下载，`-j` 控制同时下载的曲目数，`--max-connections` 控制全局连接上限 |
| 稳定性 | 分片级重试 + 指数退避、空闲超时、多镜像（backup_url）按健康度切换，PCDN/MCDN 节点自动后置 |
| 断点续传 | 每完成一个分片即记录到 `<文件>.part.state`；中断（含 Ctrl-C）后重跑同一命令即可续传，即使 CDN 链接已重新签名 |
| 搜索 | `dc search` 全屏选择界面：方向键移动、空格多选（跨页保留）、翻页、改关键词、切换排序 / 音乐区，Enter 下载；不支持全屏的终端用 `--plain` 行模式（`TERM=dumb` 或初始化失败时自动回退） |
| 输入 | BV/av 号、视频链接（含 `?p=`）、短链、合集链接、收藏夹（`fav:<id>`、`ml<id>`、favlist 链接） |

## 安装

从 [Releases](https://github.com/badawsome/downloader/releases) 下载对应平台的 `dc`，或自行编译：

```sh
cargo install --path bin/downloader-cli   # 只需要 Rust 工具链
```

## 使用

示例中的 `VIDEO_URL`、`SHORT_URL`、`COLLECTION_URL` 需替换为你要下载的视频、短链和合集链接。

```sh
# 下载单个视频的音频（未登录时最高 192K）
dc get BV1Vh4y1v7qn

# 视频链接 / 短链 / 多 P 中的某一 P
dc get "$VIDEO_URL" "$SHORT_URL"

# 下载视频所属的整个合集（--items 只取列表中的部分曲目）
dc get BV13m421J7fM --season -o ~/Music
dc get BV13m421J7fM --season --items 1-10,97

# 多 P 视频只下部分分 P
dc get BV17xxLeaEQ4 --pages 1-3,9

# 收藏夹、合集页面
dc get fav:456
dc get "$COLLECTION_URL"

# 搜索：全屏选择界面，按键即时生效
#   ↑↓ 移动 · 空格 选中 · a 全选本页 · ←→ 翻页 · / 改关键词 · o 排序 · m 只看音乐区 · Enter 下载 · q 退出
dc search 周杰伦 晴天
dc search                                                  # 直接进入界面输入关键词
dc search 一样的月光 --order plays --pick 1-3 -o ~/Music   # 不进入界面，直接下载
dc search 晴天 --plain                                     # 行模式（不支持全屏界面的终端）

# 只看会下载什么、有哪些音质（* 为将被选中的）
dc info BV1Vh4y1v7qn

# 登录（终端显示二维码，用来源 App 扫码），之后自动使用 Hi-Res / 杜比音质
dc login
dc status
dc logout
```

### 音质选择

- `-q, --quality best|hires|dolby|192k|132k|64k`：允许的最高音质，默认 `best`
  （优先级：Hi-Res FLAC > 杜比全景声 > 192K > 132K > 64K）
- `--no-dolby`：不选择杜比全景声（部分播放器不支持 E-AC-3）

### 验证能拿到的音质

`dc info <视频>` 会列出当前账号能拿到的音质（`*` 为将被选中的），以及视频提供、但当前账号拿不到的档位（`×`，例如未登录时的 Hi-Res）。
登录前后各执行一次即可对比；`dc status` 显示是否登录、是否大会员。下载后可以用 `ffprobe 文件` 查看编码（`flac` / `eac3` / `aac`），文件标签 `AUDIO_QUALITY` 也会记录所用音质。

### 文件命名

默认模板：

| 来源 | 模板 |
| --- | --- |
| 单个视频 | `{artist} - {title}` |
| 多 P 视频 / 合集 | `{album}/{index} - {title}` |
| 收藏夹 | `{collection}/{artist} - {title}` |

用 `-n, --name` 自定义，`/` 表示子目录，例如 `-n "{artist}/{album}/{index}. {title} [{bvid}]"`。
可用变量：`{title} {artist} {album} {collection} {video_title} {part} {index} {total} {page} {bvid} {aid} {cid} {date} {year}`。
扩展名按编码自动决定（`.m4a` / `.flac`）。

默认会用来源识别出的 BGM 曲名作为标题（翻唱视频常常更干净；会去掉 `发现《…》` 包装和 `(伴奏)` 等后缀），不需要时加 `--no-bgm-title`。
多 P 视频的分 P 标题若自带与序号重复的编号（如 `001.周杰伦-爱在西元前`）会自动去掉。

### 进度显示

终端中显示每首曲目的进度条（大小、速度、剩余时间、音质，重试次数以 `↻n` 标出）和总进度（曲目数、总下载量、实时速度、预计剩余时间）；解析大合集、查询曲名时显示进度提示。列宽随终端宽度自适应、中日韩字符按显示宽度对齐；输出到管道或日志时自动去掉颜色和动画，只保留每首的结果行。`-v` 会额外打印每次重试的原因。

### 并发与续传

- `-j, --jobs 3`：同时下载的曲目数
- `-c, --connections 4`：每个文件的分片连接数
- `--max-connections 16`：全局连接上限
- `--chunk-size <KiB>`：分片大小（默认按文件大小自动选择 512 KiB–8 MiB）
- `--retries 5`：每个分片的重试次数
- 已存在的目标文件会跳过，`--overwrite` 强制重新下载

### 登录信息

优先级：`--cookie "SESSDATA=..."` / 环境变量 `DC_COOKIE` > `--cookie-file`（Cookie 头或 Netscape `cookies.txt`）> `dc login` 保存的会话。
会话保存在系统配置目录（`~/.config/dc`、`%APPDATA%\dc`、`~/Library/Application Support/dc`），可用 `DC_CONFIG_DIR` 修改。

`DC_API_BASE` / `DC_PASSPORT_BASE` 可替换 API 地址（反向代理或测试用）。

## 项目结构

```
bin/downloader-cli        dc 命令行：参数、进度条、二维码登录、会话存储
service/download-service 下载编排：输入解析 → 曲目/元数据 → 命名 → 选流 → 下载 → 封装打标签
crates/source-api        来源 API：WBI 签名、Cookie/扫码登录、视频/合集/收藏夹、音频流发现与音质选择
crates/network-tools     通用下载引擎：Range 分片并行、重试、镜像切换、断点续传
crates/media             纯 Rust MP4/fMP4 处理：抽取音频轨、重封装为 M4A、FLAC 解封装、写 iTunes/Vorbis 标签与封面
```

## 开发

```sh
cargo test --workspace            # 离线测试（装有 ffmpeg 时会额外做解码校验）
cargo test -p source-api -- --ignored   # 访问真实来源 API 的测试
```


## 自动发布

版本发布由 GitHub Actions 完成，无需在本机执行 `cargo publish`。

1. 将 `bin/downloader-cli/Cargo.toml` 与 `crates/source-api/Cargo.toml` 的版本同时更新，完成合并。
2. 为该提交创建并推送相同版本的标签，例如两个版本均为 `0.2.0` 时：

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

3. 工作流先校验标签和版本、运行测试，再构建全部九个平台的安装包。
4. 全部构建成功后，自动将 `source-api` 上传到 crates.io；随后公开 GitHub Release，附安装包、逐文件 SHA-256 和 `SHA256SUMS`。

`v0.3.0-rc.1` 这类标签自动标记为 GitHub 预发布。普通合并、PR 和手动检查不会触发发布。失败后可重跑同一个 tag 的工作流；若该版本的 crate 已发布，只有包的 SHA-256 完全一致时才会跳过上传，不覆盖已经发布的不同内容。

### crate 发布授权（只需设置一次）

首次发布新名称时，在 crates.io 创建有权发布 `source-api` 的 API token，并存入本仓库 GitHub Actions secret `CARGO_REGISTRY_TOKEN`。首次上传仍由 tag 工作流执行，无需手动 `cargo publish`。不要将 token 写入源码或文档。

首次成功后，也可以改用 [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing)：仓库 owner 为 `badawsome`，仓库名 `downloader`，workflow 文件名 `release.yml`，环境名留空。配置完成后删除上述 secret，工作流会自动使用 OIDC 获取短期发布凭据。

本流程发布的库名称为 `source-api`。包名属于 crates.io 的独立身份；改名不会迁移旧名称下的版本或使用方。其他 workspace 包当前只用于构建，不在此流程中上传到 crates.io。
