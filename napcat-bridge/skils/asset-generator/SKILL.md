---
name: asset-generator
description: Generate and maintain PulseBridge heart-rate avatar assets that render live BPM in NapCat. Use when artwork, avatar.json, text or digit sprites, zone variants, or local avatar previews need to be created or revised.
metadata:
  short-description: Generate PulseBridge heart-rate avatar assets
---

# PulseBridge 心率头像资产生成器

为 `napcat-bridge` 生成一套可被 Rust 渲染器持续更新的头像资产。最终资产由两部分组成：

- `background.png`：完整的无文字、无数字底图。
- `avatar.json`：描述底图、心率显示方式、字体或数字 sprite、效果和可选分区覆写的 manifest。

这个 skill 处理的是素材和 manifest，不负责调用 QQ/NapCat API。除非用户明确要求并确认风险，不要运行会修改真实 QQ 状态、昵称或头像的集成测试。

## 项目契约

项目代码和 schema 是事实来源；文档与代码冲突时，以以下文件为准并先检查实际版本：

- `README.md`
- `assets/heart-rate/avatar.schema.json`
- `assets/heart-rate/avatar.json`
- `src/avatar.rs`
- `src/bin/pulsebridge-avatar.rs`

运行时约定：

- 部署服务固定读取 `/opt/pulsebridge/assets/heart-rate/avatar.json`；manifest 的相对路径相对于 manifest 所在目录解析。
- 输入底图必须统一为 `1280×1280` 的正方形 PNG。原则上角色构图、数字承载面设计、参数测量和底图生成都必须直接在 1280×1280 画布上完成，不经过低分辨率中间稿。1280px 源图正好对应渲染器的内部 1280px master；运行时只会从 master 缩放一次到 `PB_AVATAR_SIZE`（默认 `320`）并编码为 JPEG。
- `region.cx/cy/width/height` 和 `font_size` 使用源图坐标/尺寸。当前标准源图就是 1280px，因此这些值直接按 1280×1280 画布测量，并直接对应 master 坐标。
- 最终图层顺序是：`background` → 心率数字 → 可选 `foreground`。foreground 会盖在数字上，可用于遮挡关系或前景装饰。
- 服务和本地 CLI 使用同一套分区解析逻辑。心率区间来自运行时环境变量或 CLI 参数，不写入 `avatar.json`。
- 运行时除了数字 BPM，还会用基础配置渲染 `--`（在线但无样本）和 `OFF`（离线/断开）。如果要让这两种状态可见，优先使用 text mode，或在 sprite mode 同时提供 text fallback。

## 输入与输出

用户的美术要求放在 `{{CUSTOM_PROMPT}}`；输出目录放在 `{{OUTPUT_DIR}}`；可用且已验证的字体文件列表放在 `{{FONT_CATALOG}}`。

输出必须是：

```text
{{OUTPUT_DIR}}/background.png
{{OUTPUT_DIR}}/avatar.json
```

如果用户要求“先生成图片让我看”，先只生成底图预览并等待反馈；用户确认后再测量数字区域并写 `avatar.json`。否则在同一轮完成底图、manifest、预览和校验。

## 1. 生成底图

先从 `{{CUSTOM_PROMPT}}` 提取角色、世界观、构图、配色、材质和已有道具，再选择一个自然的数字承载面。优先选择本来就属于角色的对象，例如胸前核心、吊坠、装甲、徽章、手持道具或衣物表面。不要为了放数字凭空制造显示器。

底图必须首先是一张完整的社交头像，而不是医疗监护 UI。数字承载面应满足：

- 靠近主体但不遮挡脸、眼睛、嘴和关键动作。
- 有连续且足够大的可见表面，能容纳 `7`、`70`、`138`、`188`、`200`。
- 有稳定的局部颜色和中低频细节，缩小到约 `64×64` 后仍有轮廓。
- 能解释数字的材质来源：发光、雕刻、压印、投影、能量纹理或印刷均可。

不要生成空白的数字预留区、白/黑色矩形、标签框、HUD、数据面板、假屏幕或明显的 UI 卡片。承载区域应保留正常的材质、阴影、渐变和光照，只让纹理频率适合后续叠字。

生成图像时明确要求模型不要绘制任何可读或模糊的数字、字母、BPM、HR、Pulse、测试文字、装饰数字或辅助线。底图单独观看时不能像缺了一段文字。

优先输出无透明边缘问题的正方形 PNG。若使用 image generation 工具，生成后必须用图像查看工具检查实际结果；不要相信生成提示中的预设坐标。

## 2. 从最终底图测量显示区域

只在最终 `background.png` 生成并确认后测量参数。用图像查看工具重新观察实际图像，并在源图坐标系中记录承载面的安全矩形：

```json
{
  "cx": 640,
  "cy": 640,
  "width": 380,
  "height": 180,
  "rotation": 0
}
```

字段含义：`cx/cy` 是中心点，`width/height` 是数字层的安全区域，`rotation` 是整个数字层的角度，单位分别为 px 和 degree。1280px 源图的画布中心是 640，但不得因此默认使用 640；必须按实际承载物测量。

测量时同时判断：语义归属、是否遮挡主体、三位数空间、局部对比度、表面倾角、弧度以及 64px 缩略图表现。区域应留出描边、glow、阴影和旋转后的边缘余量；不要让最宽的 `188` 或 `200` 贴边。

`rotation=0` 表示水平；角度应跟随承载物，不要为了省事强制归零。若数字沿真实曲面排列，再设置轻微 `arc.curvature`；没有明显弧面就用接近 0 的值。

```json
{
  "curvature": 0.06,
  "x_scale": 1.0
}
```

`curvature` 必须在 `-1..1`；正值使数字中部向上拱，负值向下弯。`x_scale` 必须大于 0 且不超过 2，通常保持 `0.85..1.15`。如果必须严重压扁数字才放得下，应重新选择区域或重新生成底图。

## 3. 选择显示模式

默认使用 `combined` + `text`：一个 renderer 绘制完整的动态字符串，适合任意 BPM，并可自然显示 `--` 和 `OFF`。

只有在用户提供了一套完整数字素材并且数字造型必须固定时才使用 `sprite`。sprite 配置在继承完成后必须拥有 `0` 到 `9` 全部十个数字；每个数字可以是独立图片，也可以是 sprite sheet 的 `rect`。sprite mode 只对纯数字标签使用 sprite；非数字标签若没有 text 配置会变成空层，因此生产头像通常应保留 text fallback。

需要逐位改变位置、大小或风格时使用 `layout: "individual"`，并配置 `positions.hundreds/tens/ones`。默认的 `hide_leading_zeroes: true` 会把一位数放在 ones、两位数放在 tens/ones；三位数占用全部位置。大多数头像应使用 combined，只有数字确实需要逐位贴合复杂几何时才使用 individual。

## 4. 选择字体与效果

字体只能来自 `{{FONT_CATALOG}}` 中真实存在的文件，或运行环境已明确提供的字体。先检查文件存在并能被字体库读取，绝不编造路径。部署时优先把字体随 manifest 一起放入资产目录并使用相对路径；若使用绝对系统路径，必须确认生产机器有同一路径。

同一套头像只使用一个固定 `font_size`，不能根据 `7`、`70`、`138`、`188`、`200` 动态缩小。选字体时优先考虑 `1/7/8/0` 的区分度、三位数宽度、小尺寸清晰度和画风协调性。

效果应从最终底图实际材质和配色推导，而不是机械套用白字、荧光绿或医疗监护配色：

- 能量/晶体：可使用高亮 fill、弱 highlight 和柔和 glow。
- 金属/装甲：可使用较深 outline、弱 inner shadow，像嵌入或蚀刻。
- 布料/纸张/木材：少用 glow，保留适度 outline 以保证缩略图可读。
- 普通表面：只使用必要的对比度和轻微阴影。

所有颜色必须是 `#RRGGBB` 或 `#RRGGBBAA`。schema 的范围也必须遵守：outline width `0..32`、glow radius `0..64`、shadow offset `-64..64`、blur `0..64`。

## 5. 写 `avatar.json`

当前 manifest 的顶层结构不是旧版的平铺 `region/font/effects`，而是以下结构。不要添加 schema 未声明的字段或注释：

```json
{
  "background": "background.png",
  "foreground": null,
  "heart_rate": {
    "layout": "combined",
    "defaults": {
      "mode": "text",
      "common": {
        "region": {
          "cx": 840,
          "cy": 848,
          "width": 390,
          "height": 188,
          "rotation": 0
        },
        "hide_leading_zeroes": true
      },
      "text": {
        "font": "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "font_size": 192,
        "arc": { "curvature": 0.06, "x_scale": 1.0 },
        "effects": {
          "fill": "#F6D989E6",
          "highlight": "#FFF8D6A0",
          "outline": { "color": "#1B234A99", "width": 2 },
          "glow": { "color": "#F6D98966", "radius": 6 },
          "inner_shadow": {
            "color": "#10162B88",
            "offset_x": 2,
            "offset_y": 3,
            "blur": 2
          }
        }
      }
    }
  }
}
```

上面的数字、颜色、坐标和字体只是结构示例；提交前必须替换为从最终底图和真实字体得出的值。示例字体在当前开发环境存在，但生产部署仍需重新确认。

规则：

- `background` 必须是非空路径；推荐使用同目录相对路径 `background.png`。
- `heart_rate.defaults`、`mode`、`common.region` 和对应的 `text`/`sprite` 是必需关系。
- text mode 必须提供完整的 `font/font_size/arc/effects`。
- sprite mode 必须提供完整的十个 digit；如果要显示 `--`/`OFF`，同时提供 text 配置作为 fallback。
- `foreground` 可以为 null；如果存在，必须有 `path`，`region` 缺省时默认覆盖整个 1280 master。
- `zones.z1` 到 `zones.z5` 是可选的递归覆写。每个 zone 可以独立覆写 background、foreground 或 heart_rate；不要把心率区间阈值写进 manifest。
- zone 覆写会分别合并并最终按 background → heart rate → foreground 渲染。空对象 `{}` 是合法的，表示继承基础配置。
- 由于 Rust manifest 解析器使用 `deny_unknown_fields`，字段拼写、嵌套位置和大小写必须严格匹配 schema。

## 6. 本地预览与验证

生产预览必须使用 Rust CLI，因为它与服务共享 manifest、分区解析、文本/sprite 渲染和 JPEG 编码逻辑。先在项目根目录执行：

```bash
cargo run --bin pulsebridge-avatar -- \
  assets/heart-rate/avatar.json \
  --bpm 7,70,138,188,200 \
  --count 5 \
  --size 320 \
  --quality 50 \
  --output /tmp/pulsebridge-avatar-preview
```

验证不同心率区间时，显式传入和服务相同的运行时参数，例如：

```bash
cargo run --bin pulsebridge-avatar -- \
  assets/heart-rate/avatar.json \
  --bpm 66,140,180 \
  --zone-algorithm max_hr --max-hr 200 \
  --size 320 --quality 50 \
  --output /tmp/pulsebridge-avatar-zones

cargo run --bin pulsebridge-avatar -- \
  assets/heart-rate/avatar.json \
  --bpm 66,170,190 \
  --zone-algorithm lactate_threshold --max-hr 200 \
  --lactate-threshold 170 \
  --size 320 --quality 50 \
  --output /tmp/pulsebridge-avatar-lthr
```

`--quality` 与 `--max-bytes` 互斥；未提供时默认质量为 50。需要验证 NapCat 限制时可使用 `--max-bytes 10k`，其中 `k/m` 是十进制，`ki/mi` 是二进制。BPM 列表会循环使用，输出文件名包含 BPM 和序号。

逐张用图像查看工具检查原图和预览，至少确认：

1. `background.png` 必须是 **1280×1280** 的正方形 PNG，独立观看完整且不含任何数字/文字。
2. `7`、`70`、`138`、`188`、`200` 都完整落在承载面内；`188` 通常是宽度压力最大的案例。
3. 数字层没有遮挡脸部和关键主体，rotation/arc 与真实物体方向一致。
4. 数字 + outline/glow/shadow 没有超出 region；64×64 观感仍先看到角色，再看到心率。
5. 数字像承载物的一部分，不像后贴的医疗 UI。
6. 所有字体、背景、foreground、sprite 路径真实存在，且相对路径以 `avatar.json` 为基准。
7. `avatar.json` 能被 `pulsebridge-avatar` 加载；schema 结构、颜色格式和数值边界均合法。

推荐同时运行：

```bash
cargo test
file assets/heart-rate/background.png
```

`tools/render_heart_rate.py` 只是在画布中央粗略叠字的旧式辅助脚本，不读取 manifest，不代表服务渲染结果；不要用它作为最终校验。`tools/make_test_avatar.py` 和 `assets/heart-rate/heart-rate-*.jpg/png` 是测试/预览资产，不要把它们误当成 manifest 的源底图。

## 7. 失败处理与交付

如果三位数字放不下、必须遮脸、数字只能靠强描边才能看清、缩略图完全不可读、承载物像专门留出的空框，重新生成或修改 `background.png`，然后从最终图片重新测量。不要通过严重压缩 `x_scale`、运行时改字号或把数字硬贴到背景空白处来掩盖问题。

完成后只报告实际产物和关键决策：

- `background.png` 路径
- `avatar.json` 路径
- 实际使用的字体
- 数字承载对象
- 数字区域的大致语义位置
- 已运行的预览/校验命令及是否通过

不要输出一份与文件不同的替代 JSON，也不要把临时预览数字写回 `background.png`。
