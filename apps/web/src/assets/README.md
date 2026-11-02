# 地图材质来源

`atlas-pigment.png`：用户已授权生图，并在2026-11-02选择第1种“自然彩绘”画面。由 Codex 使用内置 `image_gen` 工具生成，未使用CLI或外部素材。实际尺寸1254×1254，PNG，SHA-256 `0807d98def88871278269c77fbeb99e68819b995457c81931c59ddebdd5b643b`。

用途是可重复的中性颜料与纸面材质。三轴映射、镜像重复与多级过滤由实时着色器完成；图片不包含地理形状。它只影响“渲染地图”，不改变高程、生态分类、降雨、水文和任何模拟字段。加载失败使用中性纹理，仍显示完整地图。整幅AI概念图只作设计参考，不加载进生产地图。

生成提示词原文：

```text
Use case: stylized-concept
Asset type: seamless tileable neutral material texture for a real-time painted atlas shader, square 1024 by 1024.
Primary request: an evenly lit top-down close-up swatch of fine cold-pressed watercolor paper covered in subtle dry gouache pigment. Soft overlapping short organic brush dabs and very fine paper fibers, beautifully handmade, restrained and uniform rather than blotchy.
Color palette: monochrome neutral middle gray, average around RGB 128, low contrast mostly RGB 105 to 150, no tinted color.
Composition: flat material fills entire square, same density to every edge, seamless repeating opposite edges, no focal point, no directional lighting, no vignette, no border.
Constraints: ONLY an abstract painterly surface swatch. No geography, maps, mountains, trees, rivers, islands, clouds, icons, letters, text, objects, shadows or watermark. Fine detail only, no large dark stains. This is a multiplicative pigment material; geography and lighting are supplied separately by a scientific renderer.
```
