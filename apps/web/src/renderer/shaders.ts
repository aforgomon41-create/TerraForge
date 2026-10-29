/**
 * 着色器源码。
 *
 * 为什么单独放一个文件：GLSL 里有大量 {} 和 # 指令，
 * 混在 TS 模板字符串里容易和插值语法冲突。
 */

export const VERT = `#version 300 es
precision highp float;

layout(location = 0) in vec3 aCenter;
layout(location = 1) in float aHeight;
layout(location = 2) in float aBiome;
layout(location = 3) in vec2 aCorner;
layout(location = 4) in float aTemperature;  // 摄氏度
layout(location = 5) in float aPrecipitation; // 毫米/年

uniform mat4 uViewProj;
uniform float uRadius;
uniform float uHeightScale;
uniform float uHexRadius;
uniform vec3 uLightDir;

// 实时预览模式：从帧纹理采样，而不是用顶点属性
uniform bool uPreviewMode;
uniform sampler2D uFrameTex;
uniform vec2 uFrameSize;
uniform float uPreviewMinElev;
uniform float uPreviewMaxElev;
uniform bool uPlanarMode;

out vec3 vNormal;
out vec3 vWorldPos;
out float vHeight;
out float vBiome;
out float vShade;
out float vMoist;
out float vTemp;

void main() {
  vec3 n = normalize(aCenter);

  // 平面模式：格子已经在 z=1 的平面上，直接按平面坐标摆放，
  // 不做球面归一化和沿法线位移（否则会变成三维突起）。
  if (uPlanarMode) {
    // 平面模式：aCenter 已经是平面坐标（x, y, 0），
    // 角点在 xy 平面上展开。不经过任何球面运算。
    vec2 planeCenter = aCenter.xy;
    vec2 corner = aCorner * uHexRadius;
    vec3 pos = vec3(planeCenter + corner, aHeight * uHeightScale * 0.3);

    vNormal = vec3(0.0, 0.0, 1.0);
    vWorldPos = pos;
    vHeight = aHeight;
    vBiome = aBiome;
    vShade = max(dot(normalize(vec3(0.0, 0.0, 1.0)), normalize(uLightDir)), 0.0);
    vMoist = 0.5;

    gl_Position = uViewProj * vec4(pos, 1.0);
    return;
  }

  // 抬升后的位置先算，然后在这个位置重建切平面基，
  // 这样六边形在球面上是真正贴合曲率的。
  float h = aHeight * uHeightScale;
  float r = uRadius + h;
  vec3 pos = n * r;

  vec3 ref = abs(n.y) < 0.9 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
  vec3 east = normalize(cross(ref, n));
  vec3 north = cross(n, east);

  // 六边形角点：在切平面上按角度分布，再投影回球面
  float hexRadius = uRadius * uHexRadius;
  vec3 tangent = (east * aCorner.x + north * aCorner.y);
  vec3 localPos = normalize(pos / r + tangent * uHexRadius) * r;

  vNormal = n;
  vWorldPos = localPos;

  if (uPreviewMode) {
    // 预览模式：把球面方向转换成等距圆柱 UV，采样帧纹理
    vec2 uv = vec2(
      atan(n.z, n.x) / 6.2831853 + 0.5,
      acos(clamp(n.y, -1.0, 1.0)) / 3.14159265
    );
    vec4 s = texture(uFrameTex, uv);
    // R = 高程（0-1），G = 温度，B = 降水，A = 流量
    // 用真实的高程范围还原物理值
    vHeight = uPreviewMinElev + s.r * (uPreviewMaxElev - uPreviewMinElev);
    vBiome = -1.0;
    vShade = s.g;   // 温度 0-1（后端已归一化）
    vMoist = s.b;   // 降水 0-1
    vTemp = (s.g / 255.0) * 80.0 - 40.0;
  } else {
    vHeight = aHeight;
    vBiome = aBiome;
    vShade = max(dot(n, normalize(uLightDir)), 0.0);
    vMoist = 0.5;
    vTemp = aTemperature;
  }

  gl_Position = uViewProj * vec4(localPos, 1.0);
}
`

export const FRAG = `#version 300 es
precision highp float;

in vec3 vNormal;
in vec3 vWorldPos;
in float vHeight;
in float vBiome;
in float vShade;
in float vMoist;
in float vTemp;

uniform vec3 uLightDir;
uniform float uSeaLevel;
uniform int uStyle;
uniform int uLayer;       // 0=地形 1=温度 2=降水 3=生物群系 4=高程

out vec4 fragColor;

vec3 biomeColor(float b) {
  int i = int(b + 0.5);
  if (i == 0) return vec3(0.04, 0.13, 0.28);
  if (i == 1) return vec3(0.92, 0.95, 0.98);
  if (i == 2) return vec3(0.55, 0.58, 0.50);
  if (i == 3) return vec3(0.14, 0.28, 0.18);
  if (i == 4) return vec3(0.20, 0.42, 0.18);
  if (i == 5) return vec3(0.55, 0.62, 0.30);
  if (i == 6) return vec3(0.58, 0.58, 0.35);
  if (i == 7) return vec3(0.82, 0.74, 0.55);
  if (i == 8) return vec3(0.70, 0.66, 0.35);
  if (i == 9) return vec3(0.08, 0.35, 0.14);
  if (i == 10) return vec3(0.20, 0.47, 0.18);
  if (i == 11) return vec3(0.47, 0.55, 0.39);
  if (i == 12) return vec3(0.27, 0.43, 0.31);
  return vec3(0.5);
}

vec3 oceanColor(float depth) {
  float d = clamp(depth / 6000.0, 0.0, 1.0);
  vec3 shallow = vec3(0.10, 0.42, 0.55);
  vec3 deep = vec3(0.015, 0.05, 0.16);
  return mix(shallow, deep, d);
}

float contourLine(float h, float interval, float width) {
  float v = h / interval;
  float f = abs(fract(v - 0.5) - 0.5) / max(fwidth(v), 1e-5);
  return 1.0 - min(f / width, 1.0);
}

// 图层配色表
vec3 temperatureColor(float t) {
  // -40 到 40 度映射成蓝到红
  float n = clamp((t + 40.0) / 80.0, 0.0, 1.0);
  vec3 cold = vec3(0.16, 0.30, 0.62);
  vec3 mild = vec3(0.32, 0.68, 0.42);
  vec3 hot = vec3(0.82, 0.28, 0.20);
  return n < 0.5 ? mix(cold, mild, n * 2.0) : mix(mild, hot, (n - 0.5) * 2.0);
}

vec3 precipitationColor(float p) {
  // 0-3000 mm 映射成土黄到深蓝
  float n = clamp(p / 3000.0, 0.0, 1.0);
  vec3 dry = vec3(0.78, 0.68, 0.44);
  vec3 wet = vec3(0.10, 0.35, 0.62);
  return mix(dry, wet, pow(n, 0.6));
}

vec3 elevationColor(float e) {
  // 分层设色：深海底 -> 浅海 -> 平原 -> 高原 -> 雪
  if (e < -4000.0) return vec3(0.05, 0.12, 0.32);
  if (e < -1000.0) return vec3(0.10, 0.28, 0.48);
  if (e < 0.0) return vec3(0.20, 0.46, 0.60);
  if (e < 500.0) return vec3(0.30, 0.55, 0.30);
  if (e < 1500.0) return vec3(0.55, 0.62, 0.32);
  if (e < 3000.0) return vec3(0.68, 0.60, 0.40);
  if (e < 5000.0) return vec3(0.60, 0.55, 0.50);
  return vec3(0.95, 0.96, 0.98);
}

void main() {
  vec3 color;

  // 图层模式：用固定配色，不受光照影响
  if (uLayer > 0) {
    if (uLayer == 1) {
      // 温度：用真实摄氏度
      color = temperatureColor(vTemp);
    } else if (uLayer == 2) {
      // 降水：vMoist 是归一化值
      color = precipitationColor(vMoist * 3000.0);
    } else if (uLayer == 3) {
      // 生物群系：用群系配色
      color = biomeColor(vBiome);
    } else {
      // 高程：分层设色
      color = elevationColor(vHeight);
    }
    fragColor = vec4(color, 1.0);
    return;
  }


  // 预览模式：用帧里的高程/温度/降水近似着色
  if (vBiome < -0.5) {
    float e = vHeight;
    float temp = vShade;      // 0-1 归一化的温度
    float moist = vMoist;     // 0-1 归一化的降水

    if (e < 0.0) {
      // 海洋：按深度深浅
      float depth = clamp(-e / 6000.0, 0.0, 1.0);
      color = mix(vec3(0.10, 0.40, 0.52), vec3(0.02, 0.06, 0.18), depth);
    } else {
      // 陆地：温度决定冷暖色，湿度决定绿/黄，高度决定明暗
      vec3 cold = vec3(0.55, 0.60, 0.56);
      vec3 temperate = vec3(0.24, 0.42, 0.22);
      vec3 tropical = vec3(0.13, 0.36, 0.16);
      vec3 base = mix(cold, temperate, smoothstep(0.15, 0.45, temp));
      base = mix(base, tropical, smoothstep(0.55, 0.85, temp));

      // 干旱程度：湿度低则偏土黄
      vec3 arid = vec3(0.76, 0.68, 0.48);
      base = mix(arid, base, smoothstep(0.10, 0.45, moist));

      // 高度：中高处露岩，极高覆雪
      float h = clamp(e / 5000.0, 0.0, 1.0);
      base = mix(base, vec3(0.52, 0.50, 0.46), smoothstep(0.45, 0.75, h) * 0.75);
      base = mix(base, vec3(0.94, 0.96, 0.99), smoothstep(0.82, 1.0, h));

      color = base;
    }

    float diff = max(dot(vNormal, normalize(uLightDir)), 0.0);
    color *= 0.45 + diff * 0.75;
    fragColor = vec4(color, 1.0);
    return;
  }

  bool isOcean = vHeight < uSeaLevel;

  if (isOcean) {
    color = oceanColor(uSeaLevel - vHeight);
  } else {
    color = biomeColor(vBiome);
    float rel = (vHeight - uSeaLevel) / 5000.0;
    if (rel > 0.75) {
      color = mix(color, vec3(0.97, 0.98, 1.0), smoothstep(0.75, 1.1, rel));
    } else if (rel > 0.45) {
      color = mix(color, vec3(0.58, 0.55, 0.50), smoothstep(0.45, 0.8, rel) * 0.6);
    }
  }

  // 光照：环境光占比高一些，避免背光面全黑。
  // 真实星球的夜面虽然暗，但还有大气散射和月光，
  // 完全黑会让用户看不到那半边的大陆。
  float diff = max(dot(vNormal, normalize(uLightDir)), 0.0);

  // 边缘光：球体边缘稍微提亮，模拟大气散射的轮廓光
  vec3 viewDir = normalize(-vWorldPos);
  float rim = pow(1.0 - max(dot(vNormal, viewDir), 0.0), 2.5);

  float shade = 0.48 + diff * 0.72 + rim * 0.18;
  color *= shade;

  if (uStyle == 1) {
    float luma = dot(color, vec3(0.299, 0.587, 0.114));
    color = mix(vec3(luma), color, 0.75) * 1.08;
  } else if (uStyle == 2) {
    float luma = dot(color, vec3(0.299, 0.587, 0.114));
    color = mix(vec3(0.78, 0.70, 0.52) * luma * 1.6, color, 0.35);
  } else if (uStyle == 3) {
    if (!isOcean) {
      float layers = floor(vHeight / 500.0) / 10.0;
      color = mix(vec3(0.35, 0.55, 0.30), vec3(0.85, 0.80, 0.65), clamp(layers, 0.0, 1.0));
      float c = contourLine(vHeight, 200.0, 1.2);
      color = mix(color, vec3(0.15, 0.12, 0.08), c * 0.7);
    }
  }

  float vignette = 1.0 - 0.15 * pow(length(vNormal.xy), 2.0);
  color *= vignette;

  fragColor = vec4(color, 1.0);
}
`
