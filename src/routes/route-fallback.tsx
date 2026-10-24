/**
 * 路由懒加载期间的占位组件。
 *
 * 设计约束（都是解释「为什么」）：
 * - 必须轻量：占位组件本身在首屏就要同步解析，任何新依赖都会抵消懒加载省下的解析开销，
 *   所以只用 Chakra 的 Box / Center / Spinner（这些已经躺在入口 chunk 里了），不引入图标或动画库。
 * - 必须占住与页面一致的高度和底色：chunk 到达前内容区如果塌成一条细线、到了再撑开，
 *   视觉上就是一次明显跳动。这里给 minH="60vh"，比任何页面的首屏内容都不矮，只多不少。
 * - 主窗口的占位只覆盖内容区：MainLayout 的标题栏 / 侧边栏在 Suspense 之外，
 *   切页面时它们始终可见，不会整窗白屏。
 */
import { Component, type ErrorInfo, type ReactNode } from "react";
import { Box, Button, Center, Spinner, Text } from "@chakra-ui/react";

/** 主窗口路由页占位：透明底 + 居中 Spinner（颜色继承正文，跟随深浅色主题） */
export function RouteFallback() {
  return (
    <Center minH="60vh" w="100%">
      <Spinner size="lg" thickness="3px" />
    </Center>
  );
}

/**
 * 整窗透明 / 鼠标穿透的独立窗口占位（灵动岛、桌面歌词、竖排悬浮框、壁纸窗）。
 *
 * 这几个窗口在 index.html 里就要求首帧整窗透明，画 Spinner 等于在透明的窗口正中
 * 闪出一个图形，比什么都不显示更突兀；一层空透明容器既不闪，也不会挡住鼠标穿透。
 */
export function TransparentWindowFallback() {
  return <Box w="100vw" h="100vh" bg="transparent" />;
}

/** 有不透明底色的独立窗口占位（传感器监控窗口）：居中 Spinner 就够 */
export function WindowFallback() {
  return (
    <Center w="100vw" h="100vh">
      <Spinner size="lg" thickness="3px" />
    </Center>
  );
}

/* ============================================================================
 * 懒加载失败兜底（ErrorBoundary）
 *
 * 为什么必须有：改成 React.lazy 之后，页面代码变成**运行期动态取 chunk**。
 * 取失败（安装包不完整、杀软拦截本地文件读取、磁盘坏道、升级只替换了部分文件）
 * 会在 render 阶段抛错；没有边界时 React 会卸载整棵树 —— 用户看到的是整窗白屏，
 * 而且连侧边栏都没了，无法切到别的页面自救。
 * 加了这个边界后：只有内容区变成一行错误提示，标题栏/侧边栏照常可用，用户能切走。
 *
 * 为什么用 class 组件：React 目前只支持 class 组件实现 componentDidCatch /
 * getDerivedStateFromError，这是唯一的写法，不是风格选择。
 * 为什么不引第三方 error-boundary 库：一行代码的事，不值得为它增加依赖。
 * ========================================================================== */

interface ErrorBoundaryProps {
  children: ReactNode;
  /** 区域名，仅用于提示文案与日志定位（如「路由页面」「灵动岛窗口」） */
  area?: string;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/** 内容区错误兜底：保持与 RouteFallback 一致的占位高度，避免报错后布局塌陷 */
export class RouteErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // 保留到控制台：桌面端 release 关闭了 DevTools，但日志仍会进 WebView2 的
    // 用户数据目录；更重要的是别让异常静默消失，否则用户反馈"某页打不开"时无从查起。
    console.error(`[RouteErrorBoundary] ${this.props.area ?? "页面"}渲染失败:`, error, info);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children as ReactNode;
    return (
      <Center minH="60vh" w="100%" flexDirection="column" gap={3} px={6} textAlign="center">
        <Text fontWeight="bold">{this.props.area ?? "页面"}加载失败</Text>
        <Text fontSize="sm" opacity={0.7} maxW="480px">
          {error.message || "未知错误"}
        </Text>
        <Text fontSize="xs" opacity={0.5}>
          可尝试切换到其它页面后返回，或重新启动 NeoBox。
        </Text>
        <Button size="sm" onClick={this.handleRetry}>
          重试
        </Button>
      </Center>
    );
  }

  /**
   * 重试。
   *
   * **为什么不能只 `setState({ error: null })`**：`React.lazy` 对失败的
   * `import()` 会缓存 reject 结果（React 内部记 promise 状态），
   * 同一个 lazy 组件再次渲染时直接复用那个已失败的 promise，
   * 于是「清空错误 → 重新渲染」只会立刻抛同一个错，按钮点了没反应。
   *
   * 真正有效的重试有两条路：
   * 1. 重新加载整个页面（`location.reload()`）：chunk 会重新请求，能恢复的都能恢复。
   *    桌面端从本地磁盘取 chunk，正常情况几乎不会失败，走到这里基本意味着
   *    安装不完整/文件被拦，重载是唯一可能自愈的办法。
   * 2. 若重载后仍失败，说明文件确实缺失，只能提示用户。
   *
   * 先清空错误状态再重载：这样即使 reload 被拦截（极端情况），
   * 用户也能看到页面重新尝试渲染，而不是停在一个"死"的按钮上。
   */
  private handleRetry = () => {
    this.setState({ error: null });
    try {
      window.location.reload();
    } catch {
      /* reload 被阻止：保留已清空的状态，让用户可手动切页重试 */
    }
  };
}
