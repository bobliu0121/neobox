import { Box, Flex, Text, HStack, VStack, Spinner, useColorModeValue } from "@chakra-ui/react";
import { useState, useEffect, useRef, useCallback } from "react";
import { Monitor, Camera, AlertTriangle } from "lucide-react";

/** 单个采集图层的布局（画布像素坐标） */
export interface LayerLayout {
  x: number;
  y: number;
  w: number;
  h: number;
  crop_x: number;
  crop_y: number;
  crop_w: number;
  crop_h: number;
  visible: boolean;
}

/** 交互模式 */
type DragMode =
  | "none"
  | "move"
  | "resize-tl"
  | "resize-tr"
  | "resize-bl"
  | "resize-br"
  | "crop";

/** 角句柄位置 */
const CORNERS = ["tl", "tr", "bl", "br"] as const;
type Corner = (typeof CORNERS)[number];

interface CanvasPreviewProps {
  canvasW: number;
  canvasH: number;
  screenLayout: LayerLayout;
  cameraLayout: LayerLayout;
  screenEnabled: boolean;
  cameraDevice: string;
  /** 屏幕源的 MJPEG 流地址 */
  screenStreamUrl: string;
  /** 摄像头源的 MJPEG 流地址 */
  cameraStreamUrl: string;
  activeLayer: "screen" | "camera" | null;
  onActiveLayerChange: (layer: "screen" | "camera" | null) => void;
  onScreenLayoutChange: (layout: LayerLayout) => void;
  onCameraLayoutChange: (layout: LayerLayout) => void;
  disabled?: boolean;
  previewState?: "idle" | "starting" | "running" | "error";
  previewError?: string;
  previewEnabled?: boolean;
  /** 屏幕源预览失败原因（空 = 正常） */
  screenPreviewError?: string;
  /** 摄像头源预览失败原因（空 = 正常） */
  cameraPreviewError?: string;
}

/**
 * 可拖拽的分层合成画布。
 *
 * # 为什么预览在前端合成
 *
 * 早期让后端 ffmpeg 合成整幅画面，导致任何布局变化都要重启 ffmpeg 进程，
 * 拖动图层时画面要等几百毫秒才更新 —— 用户感受是「拖过去了画面才跳过去」。
 *
 * 现在每个采集源各输出一路原始 MJPEG 流，前端用两条 <img> 叠加并按
 * 图层布局做 CSS 变换。拖拽只改 CSS 内联样式，浏览器合成层下一帧就重绘，
 * 真正做到「手指在动，画面就跟」。
 *
 * 录制成片仍由后端 ffmpeg 按同一套布局参数合成，所以预览所见即录所得。
 */
export default function CanvasPreview({
  canvasW,
  canvasH,
  screenLayout,
  cameraLayout,
  screenEnabled,
  cameraDevice,
  screenStreamUrl,
  cameraStreamUrl,
  activeLayer,
  onActiveLayerChange,
  onScreenLayoutChange,
  onCameraLayoutChange,
  disabled = false,
  previewState = "idle",
  previewError = "",
  previewEnabled = false,
  screenPreviewError = "",
  cameraPreviewError = "",
}: CanvasPreviewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [displayW, setDisplayW] = useState(0);
  const [dragMode, setDragMode] = useState<DragMode>("none");
  const [dragLayer, setDragLayer] = useState<"screen" | "camera" | null>(null);
  const [dragStart, setDragStart] = useState({
    mx: 0, my: 0, x: 0, y: 0, w: 0, h: 0, cx: 0, cy: 0, cw: 0, ch: 0,
  });

  const borderColor = useColorModeValue("gray.300", "rgba(255,255,255,0.2)");
  const canvasBg = useColorModeValue("#111", "#0a0a0a");
  const activeColor = useColorModeValue("blue.500", "#4B8BD6");
  const labelBg = useColorModeValue("rgba(0,0,0,0.75)", "rgba(0,0,0,0.8)");

  // 按容器宽度等比缩放画布
  useEffect(() => {
    const measure = () => {
      if (!containerRef.current) return;
      const avail = containerRef.current.clientWidth;
      if (avail > 0) setDisplayW(avail);
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (containerRef.current) ro.observe(containerRef.current);
    return () => ro.disconnect();
  }, []);

  const scale = displayW > 0 && canvasW > 0 ? displayW / canvasW : 0;
  const displayH = canvasH * scale;

  const toCanvasDelta = useCallback(
    (dxPx: number, dyPx: number) => {
      if (scale <= 0) return { dx: 0, dy: 0 };
      return { dx: Math.round(dxPx / scale), dy: Math.round(dyPx / scale) };
    },
    [scale]
  );

  const startDrag = useCallback(
    (e: React.MouseEvent, layer: "screen" | "camera", mode: DragMode) => {
      if (disabled) return;
      e.preventDefault();
      e.stopPropagation();
      const layout = layer === "screen" ? screenLayout : cameraLayout;
      setDragLayer(layer);
      setDragMode(mode);
      setDragStart({
        mx: e.clientX, my: e.clientY,
        x: layout.x, y: layout.y, w: layout.w, h: layout.h,
        cx: layout.crop_x, cy: layout.crop_y, cw: layout.crop_w, ch: layout.crop_h,
      });
      onActiveLayerChange(layer);
    },
    [disabled, screenLayout, cameraLayout, onActiveLayerChange]
  );

  // 全局拖拽：不限制在画布内（允许把图层拖到画面外）
  useEffect(() => {
    if (dragMode === "none" || !dragLayer) return;

    const onMove = (e: MouseEvent) => {
      const { dx, dy } = toCanvasDelta(e.clientX - dragStart.mx, e.clientY - dragStart.my);
      const layer = dragLayer;
      const origin = layer === "screen" ? screenLayout : cameraLayout;
      const apply = (next: LayerLayout) => {
        if (layer === "screen") onScreenLayoutChange(next);
        else onCameraLayoutChange(next);
      };

      if (dragMode === "move") {
        // 不钳制：允许拖到画布外（后端 overlay 支持负坐标）
        apply({ ...origin, x: dragStart.x + dx, y: dragStart.y + dy });
        return;
      }

      if (dragMode === "crop") {
        const ncw = Math.max(32, dragStart.cw + dx);
        const nch = Math.max(32, dragStart.ch + dy);
        apply({
          ...origin,
          crop_x: Math.max(0, dragStart.cx + dx),
          crop_y: Math.max(0, dragStart.cy + dy),
          crop_w: ncw - (ncw % 2),
          crop_h: nch - (nch % 2),
        });
        return;
      }

      // 四角缩放：拖哪个角，对角的坐标保持不动
      //
      // 必须一次性算出最终的 x/y/w/h 再提交：若先改宽度再改高度分别提交，
      // 两次 setState 会互相覆盖（第二次基于旧 origin），导致缩放乱跳。
      const minW = 64;
      const minH = 36;
      const fromLeft = dragMode === "resize-tl" || dragMode === "resize-bl";
      const fromTop = dragMode === "resize-tl" || dragMode === "resize-tr";

      let nx = dragStart.x;
      let ny = dragStart.y;
      let nw = dragStart.w;
      let nh = dragStart.h;

      if (fromLeft) {
        // 左边界跟着鼠标走，右边界固定
        const right = dragStart.x + dragStart.w;
        const movedX = Math.min(right - minW, dragStart.x + dx);
        nx = movedX;
        nw = right - movedX;
      } else {
        nw = Math.max(minW, dragStart.w + dx);
      }

      if (fromTop) {
        // 上边界跟着鼠标走，下边界固定
        const bottom = dragStart.y + dragStart.h;
        const movedY = Math.min(bottom - minH, dragStart.y + dy);
        ny = movedY;
        nh = bottom - movedY;
      } else {
        nh = Math.max(minH, dragStart.h + dy);
      }

      apply({
        ...origin,
        x: nx,
        y: ny,
        w: nw - (nw % 2),
        h: nh - (nh % 2),
      });
    };

    const onUp = () => {
      setDragMode("none");
      setDragLayer(null);
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [
    dragMode, dragLayer, dragStart, toCanvasDelta,
    screenLayout, cameraLayout,
    onScreenLayoutChange, onCameraLayoutChange,
  ]);

  /** 渲染图层：源画面 + CSS 变换 + 手柄 */
  const renderLayer = (
    key: "screen" | "camera",
    layout: LayerLayout,
    streamUrl: string,
    icon: React.ReactNode,
    label: string,
    hasSource: boolean
  ) => {
    if (!hasSource) return null;
    const isActive = activeLayer === key;
    // 该源在图层的宽高：w/h 为 0 表示「自动铺满画布」，与后端规则一致
    const layerW = layout.w > 0 ? layout.w : canvasW;
    const layerH = layout.h > 0 ? layout.h : canvasH;
    const dx = layout.x * scale;
    const dy = layout.y * scale;
    const dw = layerW * scale;
    const dh = layerH * scale;
    // 该源的预览失败原因（屏幕/摄像头各记各的）
    const sourceError = key === "screen" ? screenPreviewError : cameraPreviewError;

    // 裁剪：用外层容器的 overflow hidden + 内层 img 反向偏移实现
    // 这样 css 变换即可，不需要 img 真的被裁
    const hasCrop = layout.crop_w > 0 && layout.crop_h > 0;

    return (
      <Box
        key={key}
        position="absolute"
        left={`${dx}px`}
        top={`${dy}px`}
        width={`${dw}px`}
        height={`${dh}px`}
        overflow="hidden"
        border="2px solid"
        borderColor={isActive ? activeColor : borderColor}
        borderRadius="2px"
        opacity={layout.visible ? 1 : 0.35}
        cursor={disabled ? "default" : "move"}
        onMouseDown={(e) => startDrag(e, key, "move")}
        zIndex={isActive ? 30 : 20}
        // 拖拽时关掉过渡，保证画面紧跟鼠标；非拖拽时给边框一点动画
        transition={dragMode === "none" ? "border-color 0.15s" : "none"}
        _hover={{ borderColor: activeColor }}
        sx={{ willChange: "left, top, width, height" }}
      >
        {/* 源画面 */}
        {streamUrl ? (
          <img
            src={streamUrl}
            alt={label}
            draggable={false}
            style={{
              position: "absolute",
              // 裁剪偏移：把「整幅源」按裁剪区放大并左移，实现内容裁剪
              left: hasCrop ? `${-(layout.crop_x / Math.max(layout.crop_w, 1)) * 100}%` : 0,
              top: hasCrop ? `${-(layout.crop_y / Math.max(layout.crop_h, 1)) * 100}%` : 0,
              width: hasCrop ? `${(layerW / Math.max(layout.crop_w, 1)) * 100}%` : "100%",
              height: hasCrop ? `${(layerH / Math.max(layout.crop_h, 1)) * 100}%` : "100%",
              // contain：等比缩放 + 留黑边，与后端 scale(force_original_aspect_ratio)+pad 一致。
              // 用 fill 会把采集源拉扁拉长，改画布比例时画面会变形。
              objectFit: "contain",
              pointerEvents: "none",
              userSelect: "none",
            }}
          />
        ) : (
          <Box
            position="absolute"
            inset={0}
            bg={key === "screen" ? "rgba(59,130,246,0.18)" : "rgba(16,185,129,0.18)"}
            border="1px dashed"
            borderColor="rgba(255,255,255,0.25)"
            display="flex"
            alignItems="center"
            justifyContent="center"
            px={2}
          >
            {/* 该源具体的失败原因：屏幕正常但摄像头失败时，
                用户能在摄像头这块区域直接看到原因，不必去别处找 */}
            {sourceError ? (
              <VStack spacing={0.5} textAlign="center">
                <AlertTriangle size={14} color="orange.400" />
                <Text fontSize="2xs" color="orange.300" noOfLines={3}>
                  {sourceError}
                </Text>
              </VStack>
            ) : (
              <Text fontSize="2xs" color="rgba(255,255,255,0.45)">
                {label}无画面
              </Text>
            )}
          </Box>
        )}

        {/* 图层标签 */}
        <HStack
          position="absolute"
          top="2px"
          left="2px"
          spacing={1}
          px={1.5}
          py={0.5}
          borderRadius="3px"
          bg={labelBg}
          color="white"
          fontSize="2xs"
          pointerEvents="none"
        >
          {icon}
          <Text>{label}</Text>
          {!layout.visible && <Text opacity={0.7}>(隐藏)</Text>}
        </HStack>

        {/* 四角缩放手柄 */}
        {!disabled &&
          CORNERS.map((corner) => {
            const pos: Record<Corner, React.CSSProperties> = {
              tl: { left: -6, top: -6, cursor: "nwse-resize" },
              tr: { right: -6, top: -6, cursor: "nesw-resize" },
              bl: { left: -6, bottom: -6, cursor: "nesw-resize" },
              br: { right: -6, bottom: -6, cursor: "nwse-resize" },
            };
            return (
              <Box
                key={corner}
                position="absolute"
                w="12px"
                h="12px"
                borderRadius="2px"
                bg={activeColor}
                border="1px solid rgba(255,255,255,0.6)"
                style={pos[corner]}
                zIndex={40}
                onMouseDown={(e) => startDrag(e, key, `resize-${corner}` as DragMode)}
                _hover={{ transform: "scale(1.15)" }}
              />
            );
          })}

        {/* 裁剪手柄（黄色，放在底边中点，避免和四角缩放混淆） */}
        {!disabled && (
          <Box
            position="absolute"
            left="50%"
            bottom="-6px"
            w="14px"
            h="14px"
            mt="-7px"
            ml="-7px"
            borderRadius="full"
            bg="yellow.400"
            border="1px solid rgba(0,0,0,0.4)"
            cursor="crosshair"
            zIndex={40}
            onMouseDown={(e) => startDrag(e, key, "crop")}
            _hover={{ transform: "scale(1.15)" }}
          />
        )}
      </Box>
    );
  };

  if (displayW === 0) {
    return <Box ref={containerRef} w="full" h="200px" />;
  }

  const anyLive = (screenEnabled && screenStreamUrl) || (cameraDevice && cameraStreamUrl);

  return (
    <VStack align="stretch" spacing={2} w="full">
      <Box
        ref={containerRef}
        position="relative"
        w="full"
        bg={canvasBg}
        borderRadius="md"
        border="1px solid"
        borderColor={borderColor}
        overflow="hidden"
        sx={{ aspectRatio: canvasW > 0 && canvasH > 0 ? `${canvasW} / ${canvasH}` : undefined }}
        minH={`${displayH}px`}
        onMouseDown={() => onActiveLayerChange(null)}
      >
        {/* 无任何画面时给出准确状态 */}
        {!anyLive && (
          <VStack
            position="absolute"
            inset={0}
            align="center"
            justify="center"
            spacing={1}
            color="gray.500"
            px={4}
            textAlign="center"
            pointerEvents="none"
            zIndex={10}
          >
            {previewState === "error" ? (
              <>
                <AlertTriangle size={22} opacity={0.6} />
                <Text fontSize="2xs" color="orange.300">预览失败</Text>
                {previewError && (
                  <Text fontSize="2xs" opacity={0.75} wordBreak="break-all">{previewError}</Text>
                )}
              </>
            ) : previewEnabled && previewState === "starting" ? (
              <>
                <Spinner size="sm" />
                <Text fontSize="2xs">预览启动中…</Text>
              </>
            ) : previewEnabled && previewState === "running" ? (
              <>
                <Monitor size={22} opacity={0.4} />
                <Text fontSize="2xs">等待画面…</Text>
              </>
            ) : previewEnabled ? (
              <>
                <Monitor size={22} opacity={0.4} />
                <Text fontSize="2xs">预览未启动</Text>
              </>
            ) : (
              <>
                <Monitor size={22} opacity={0.4} />
                <Text fontSize="2xs">预览已关闭</Text>
              </>
            )}
          </VStack>
        )}

        {/* 图层（屏幕在下、摄像头在上） */}
        {renderLayer("screen", screenLayout, screenEnabled ? screenStreamUrl : "", <Monitor size={10} />, "屏幕", screenEnabled)}
        {renderLayer("camera", cameraLayout, cameraDevice !== "" ? cameraStreamUrl : "", <Camera size={10} />, "摄像头", cameraDevice !== "")}
      </Box>

      <HStack justify="space-between" fontSize="2xs" color="gray.500" px={1}>
        <Text>画布 {canvasW}×{canvasH}</Text>
        <Text>显示比例 {scale > 0 ? `${(scale * 100).toFixed(1)}%` : "-"}</Text>
      </HStack>
    </VStack>
  );
}
