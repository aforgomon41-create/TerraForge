// ============================================================
// 旧版（球面网格 v2）面内投影，仅用于兼容已有工程
// ============================================================
//
// 此实现原先标为 Snyder 等面积投影，但面内映射在边缘的极限与
// 规范 slerp 棱点并不一致；高分辨率下会产生细长三角形和可见接缝。
// 不再将其视为经过验证的 Snyder 实现，也不再用于新生成的网格。
// 保留计算结果是为了让 v2 工程的数据继续对应原来的地理坐标。
// 新网格使用 sphere_hex 中的对称球面插值，并直接计算每格物理面积。

use glam::DVec3;

/// 球面三角形的 Snyder 正向投影：球面点 -> 平面重心坐标。
///
/// 输入三个球面顶点 v0, v1, v2（逆时针）和球面点 p。
/// 输出 (beta0, beta1, beta2)，满足和为 1，对应平面三角形上的位置。
pub fn snyder_forward(v0: DVec3, v1: DVec3, v2: DVec3, p: DVec3) -> [f64; 3] {
    // p 是 v0，退化情况直接返回
    if (p - v0).length() < 1e-15 {
        return [1.0, 0.0, 0.0];
    }

    // 平面三角形用单位直角三角形，面积为 1/2
    // 对应的面积函数 A(a,b,c) = 有向面积
    let area_total = spherical_triangle_area(v0, v1, v2);
    if area_total.abs() < 1e-15 {
        return [1.0, 0.0, 0.0];
    }

    // 找到大圆 (v0,p) 和 (v1,v2) 的交点 p_hat
    let n1 = v0.cross(p);
    let n2 = v1.cross(v2);
    let mut p_hat = n1.cross(n2);
    let len = p_hat.length();
    if len < 1e-15 {
        // 平行，退化为到 v1-v2 边的直接投影
        return [1.0, 0.0, 0.0];
    }
    p_hat /= len;
    // 选正确的那个交点（两个交点里离 v0 近的）
    if p_hat.dot(v0) < 0.0 {
        p_hat = -p_hat;
    }

    // h = sqrt((1 - v0·p) / (1 - v0·p_hat))
    let num = 1.0 - v0.dot(p);
    let den = 1.0 - v0.dot(p_hat);
    if den.abs() < 1e-15 {
        return [1.0, 0.0, 0.0];
    }
    let h = (num / den).max(0.0).sqrt();

    // beta2 = h * A(v0, v1, p_hat) / A(v0, v1, v2)
    let area_sub = spherical_triangle_area(v0, v1, p_hat);
    let beta2 = h * area_sub / area_total;
    let beta0 = 1.0 - h;
    let beta1 = 1.0 - beta0 - beta2;

    [beta0, beta1, beta2]
}

/// 球面三角形的有向面积（立体角），逆时针为正。
fn spherical_triangle_area(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    // 用 l'Huilier 定理的变体，对球面三角形精确
    let numerator = a.dot(b.cross(c));
    let denominator = 1.0 + a.dot(b) + b.dot(c) + c.dot(a);
    2.0 * numerator.atan2(denominator)
}

/// Snyder 逆投影：平面重心坐标 -> 球面点。
///
/// beta 是 (beta0, beta1, beta2)，对应平面三角形上的重心坐标。
pub fn snyder_inverse(v0: DVec3, v1: DVec3, v2: DVec3, beta: [f64; 3]) -> DVec3 {
    let (beta0, beta1, beta2) = (beta[0], beta[1], beta[2]);

    // 顶点上的情况直接返回
    if beta0 >= 1.0 - 1e-12 {
        return v0;
    }
    if beta1 >= 1.0 - 1e-12 {
        return v1;
    }
    if beta2 >= 1.0 - 1e-12 {
        return v2;
    }
    if beta0 <= 1e-12 {
        // 落在 v1-v2 边上，用 slerp
        let t = beta2 / (beta1 + beta2);
        return slerp(v1, v2, t);
    }

    let h = 1.0 - beta0;
    let area_total = spherical_triangle_area(v0, v1, v2);
    let a = (beta2 / h) * area_total;

    let c01 = v0.dot(v1);
    let c12 = v1.dot(v2);
    let c20 = v2.dot(v0);
    let s12 = (1.0 - c12 * c12).max(0.0).sqrt();
    let det_v = v0.dot(v1.cross(v2));

    let sin_a = a.sin();
    let cos_a_m = 1.0 - a.cos(); // 1 - cos(a)

    let f = sin_a * det_v + cos_a_m * (c01 * c12 - c20);
    let g = cos_a_m * s12 * (1.0 + c01);

    // q = (2 / arccos(c12)) * atan(g / f)
    let angle_12 = c12.clamp(-1.0, 1.0).acos();
    let q = if angle_12.abs() < 1e-15 {
        0.0
    } else {
        (2.0 / angle_12) * g.atan2(f)
    };

    // p_hat 是 v1-v2 边上的点
    let p_hat = slerp(v1, v2, q.clamp(0.0, 1.0));

    // t = arccos(1 + h²(v0·p_hat - 1)) / arccos(v0·p_hat)
    let dot0p = v0.dot(p_hat).clamp(-1.0, 1.0);
    let inner = (1.0 + h * h * (dot0p - 1.0)).clamp(-1.0, 1.0);
    let angle_0p = dot0p.acos();
    let t = if angle_0p.abs() < 1e-15 {
        0.0
    } else {
        inner.acos() / angle_0p
    };

    slerp(v0, p_hat, t.clamp(0.0, 1.0)).normalize()
}

/// 球面线性插值。
fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let dot = a.dot(b).clamp(-1.0, 1.0);
    let theta = dot.acos();
    if theta.abs() < 1e-12 {
        return a;
    }
    let sin_theta = theta.sin();
    let w0 = ((1.0 - t) * theta).sin() / sin_theta;
    let w1 = (t * theta).sin() / sin_theta;
    a * w0 + b * w1
}
