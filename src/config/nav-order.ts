/**
 * 导航栏默认顺序与可隐藏项。
 *
 * 顺序包含「主页」「设置」：它们不允许隐藏，但允许调整位置。
 * 其余项既可隐藏，也可在设置中拖动排序。
 */
export const DEFAULT_NAV_ORDER = [
  "/",
  "/builtin-tools",
  "/wallpaper",
  "/music",
  "/search",
  "/hardware",
  "/optimization",
  "/system-optimizer",
  "/settings",
  "/custom",
] as const;

/** 固定显示、不可隐藏的导航项 */
export const FIXED_NAV_PATHS = ["/", "/settings"] as const;

/** 可隐藏的导航项（设置页提供开关） */
export const HIDEABLE_NAV_PATHS = DEFAULT_NAV_ORDER.filter(
  (path) => !(FIXED_NAV_PATHS as readonly string[]).includes(path)
);

export function isFixedNavPath(path: string): boolean {
  return (FIXED_NAV_PATHS as readonly string[]).includes(path);
}

/** 把任意已保存的顺序补全并去重为完整顺序 */
export function normalizeNavOrder(saved: unknown): string[] {
  const parsed = Array.isArray(saved) ? saved.filter((p): p is string => typeof p === "string") : [];
  // 旧版本只保存中间项（没有主页/设置/壁纸/系统优化），无法推断新位置，直接采用新默认顺序
  if (!parsed.includes("/") || !parsed.includes("/settings")) {
    return [...DEFAULT_NAV_ORDER];
  }
  const known = DEFAULT_NAV_ORDER as readonly string[];
  const result: string[] = [];
  for (const path of parsed) {
    if (known.includes(path) && !result.includes(path)) result.push(path);
  }
  for (const path of DEFAULT_NAV_ORDER) {
    if (!result.includes(path)) result.push(path);
  }
  return result;
}
