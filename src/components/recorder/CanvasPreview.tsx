import { Box, Flex, Text, HStack, VStack, useColorModeValue } from "@chakra-ui/react";
import { useState, useEffect, useRef, useCallback } from "react";
import { Monitor, Camera, Eye, EyeOff, Move, Maximize2, Crop } from "lucide-react";

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

/** 拖拽 / 缩放 / 裁剪的交互模式 */
type DragMode = "none" | "move" | "resize" | "crop";

interface CanvasPreviewProps {
  /** 画布真实尺寸（像素，如 1920x1080） */
  canvasW: number;
  canvasH: number;
  /** 屏幕图层布局 */
  screenLayout: LayerLayout;
  /** 摄像头图层布局 */
  cameraLayout: LayerLayout;
  /** 是否启用屏幕源 */
  screenEnabled: boolean;
  /** 摄像头设备名（空表示未启用） */
  cameraDevice: string;
  /** MJPEG 预览地址（空表示不显示实时流） */
  previewUrl: string;
  /** 当前选中的图层 */
  activeLayer: "screen" | "camera" | null;
  onActiveLayerChange: (layer: "screen" | "camera" | null) => void;
  onScreenLayoutChange: (layout: LayerLayout) => void;
  onCameraLayoutChange: (layout: LayerLayout) => void;
  /** 是否处于录制中（禁用手柄交互） */
  disabled?: boolean;
  /** 是否显示实时预览画面 */
  showLive: boolean;
}

/**
 * 可拖拽的采集画布。
 *
 * 设计要点：
 * - 预览区按比例缩小显示真实画布，拖拽时把显示像素换算回画布像素再上报，
 *   所以布局在不同分辨率下都稳定（后端收到的永远是画布像素）。
 * - 三个操作手柄：整体拖拽（move）、右下角缩放（resize）、裁剪框（crop）。
 *   裁剪只改变「采集内容区域」，不改变图层在画布上的位置与显示尺寸，
 *   两者解耦后用户能分别调整「看哪一块」与「显示多大」。
 * - 实时预览用 <img> 指向后端 MJPEG 流；关掉预览时退化为示意图。
 */
export default function CanvasPreview({
  canvasW,
  canvasH,
  screenLayout,
  cameraLayout,
  screenEnabled,
  cameraDevice,
  previewUrl,
  activeLayer,
  onActiveLayerChange,
  onScreenLayoutChange,
  onCameraLayoutChange,
  disabled = false,
  showLive,
}: CanvasPreviewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [displayW, setDisplayW] = useState(0);
  const [dragMode, setDragMode] = useState<DragMode>("none");
  const [dragLayer, setDragLayer] = useState<"screen" | "camera" | null>(null);
  const [dragStart, setDragStart] = useState({ mx: 0, my: 0, x: 0, y: 0, w: 0, h: 0, cx: 0, cy: 0, cw: 0, ch: 0 });

  const borderColor = useColorModeValue("gray.300", "rgba(255,255,255,0.2)");
  const canvasBg = useColorModeValue("#1a1a1a", "#0d0d0d");
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

  // 显示比例：显示像素 / 画布像素
  const scale = displayW > 0 && canvasW > 0 ? displayW / canvasW : 0;
  const displayH = canvasH * scale;

  /** 把鼠标位移换算成画布像素 */
  const toCanvasDelta = useCallback(
    (dxPx: number, dyPx: number) => {
      if (scale <= 0) return { dx: 0, dy: 0 };
      return { dx: Math.round(dxPx / scale), dy: Math.round(dyPx / scale) };
    },
    [scale]
  );

  /** 开始拖拽 */
  const startDrag = useCallback(
    (e: React.MouseEvent, layer: "screen" | "camera", mode: DragMode) => {
      if (disabled) return;
      e.preventDefault();
      e.stopPropagation();
      const layout = layer === "screen" ? screenLayout : cameraLayout;
      setDragLayer(layer);
      setDragMode(mode);
      setDragStart({
        mx: e.clientX,
        my: e.clientY,
        x: layout.x,
        y: layout.y,
        w: layout.w,
        h: layout.h,
        cx: layout.crop_x,
        cy: layout.crop_y,
        cw: layout.crop_w,
        ch: layout.crop_h,
      });
      onActiveLayerChange(layer);
    },
    [disabled, screenLayout, cameraLayout, onActiveLayerChange]
  );

  // 全局鼠标事件：拖出画布外也能继续
  useEffect(() => {
    if (dragMode === "none" || !dragLayer) return;

    const onMove = (e: MouseEvent) => {
      const { dx, dy } = toCanvasDelta(e.clientX - dragStart.mx, e.clientY - dragStart.my);
      const layer = dragLayer;
      const origin = layer === "screen" ? screenLayout : cameraLayout;

      if (dragMode === "move") {
        // 位置：钳制在画布范围内（允许部分出界但至少留一角可见）
        const nx = Math.max(-origin.w + 40, Math.min(canvasW - 40, dragStart.x + dx));
        const ny = Math.max(-origin.h + 40, Math.min(canvasH - 40, dragStart.y + dy));
        const next = { ...origin, x: nx, y: ny };
        if (layer === "screen") onScreenLayoutChange(next);
        else onCameraLayoutChange(next);
      } else if (dragMode === "resize") {
        // 缩放：最小 64x36，保证偶数
        const nw = Math.max(64, Math.min(canvasW, dragStart.w + dx));
        const nh = Math.max(36, Math.min(canvasH, dragStart.h + dy));
        const next = {
          ...origin,
          w: nw - (nw % 2),
          h: nh - (nh % 2),
        };
        if (layer === "screen") onScreenLayoutChange(next);
        else onCameraLayoutChange(next);
      } else if (dragMode === "crop") {
        // 裁剪：起点+尺寸，钳制在源画面范围内
        const ncx = Math.max(0, dragStart.cx + dx);
        const ncy = Math.max(0, dragStart.cy + dy);
        const ncw = Math.max(32, dragStart.cw + dx);
        const nch = Math.max(32, dragStart.ch + dy);
        const next = {
          ...origin,
          crop_x: ncx,
          crop_y: ncy,
          crop_w: ncw - (ncw % 2),
          crop_h: nch - (nch % 2),
        };
        if (layer === "screen") onScreenLayoutChange(next);
        else onCameraLayoutChange(next);
      }
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
    dragMode,
    dragLayer,
    dragStart,
    toCanvasDelta,
    canvasW,
    canvasH,
    screenLayout,
    cameraLayout,
    onScreenLayoutChange,
    onCameraLayoutChange,
  ]);

  /** 渲染单个图层框 */
  const renderLayer = (
    key: "screen" | "camera",
    layout: LayerLayout,
    icon: React.ReactNode,
    label: string,
    hasSource: boolean
  ) => {
    if (!hasSource) return null;
    const isActive = activeLayer === key;
    // 显示坐标
    const dx = layout.x * scale;
    const dy = layout.y * scale;
    const dw = layout.w * scale;
    const dh = layout.h * scale;

    return (
      <Box
        key={key}
        position="absolute"
        left={`${dx}px`}
        top={`${dy}px`}
        width={`${dw}px`}
        height={`${dh}px`}
        border="2px solid"
        borderColor={isActive ? activeColor : borderColor}
        borderRadius="4px"
        bg={layout.visible ? "transparent" : "rgba(0,0,0,0.5)"}
        opacity={layout.visible ? 1 : 0.55}
        cursor={disabled ? "default" : "move"}
        onMouseDown={(e) => startDrag(e, key, "move")}
        onDoubleClick={() => onActiveLayerChange(isActive ? null : key)}
        zIndex={isActive ? 20 : 10}
        transition={dragMode === "none" ? "border-color 0.15s" : "none"}
        _hover={{ borderColor: activeColor }}
      >
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

        {/* 裁剪区域示意（虚线内框） */}
        {(layout.crop_w > 0 || layout.crop_h > 0) && (
          <Box
            position="absolute"
            left={`${(layout.crop_x / Math.max(layout.w, 1)) * 100}%`}
            top={`${(layout.crop_y / Math.max(layout.h, 1)) * 100}%`}
            width={`${(layout.crop_w / Math.max(layout.w, 1)) * 100}%`}
            height={`${(layout.crop_h / Math.max(layout.h, 1)) * 100}%`}
            border="1px dashed"
            borderColor="yellow.400"
            pointerEvents="none"
          />
        )}

        {/* 右下角缩放手柄 */}
        {!disabled && (
          <Box
            position="absolute"
            right="-6px"
            bottom="-6px"
            w="12px"
            h="12px"
            borderRadius="2px"
            bg={activeColor}
            cursor="nwse-resize"
            onMouseDown={(e) => startDrag(e, key, "resize")}
            zIndex={30}
            _hover={{ transform: "scale(1.2)" }}
          />
        )}
        {/* 左侧裁剪手柄 */}
        {!disabled && (
          <Box
            position="absolute"
            left="-6px"
            bottom="-6px"
            w="12px"
            h="12px"
            borderRadius="full"
            bg="yellow.400"
            cursor="crosshair"
            onMouseDown={(e) => startDrag(e, key, "crop")}
            zIndex={30}
            _hover={{ transform: "scale(1.2)" }}
          />
        )}
      </Box>
    );
  };

  if (displayW === 0) {
    return <Box ref={containerRef} w="full" h="200px" />;
  }

  return (
    <VStack align="stretch" spacing={2} w="full">
      {/* 画布 */}
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
        {/* 实时预览底图 */}
        {showLive && previewUrl ? (
          <img
            src={previewUrl}
            alt="preview"
            style={{
              position: "absolute",
              inset: 0,
              width: "100%",
              height: "100%",
              objectFit: "fill",
              pointerEvents: "none",
            }}
          />
        ) : (
          <Flex>
            {/* 无预览时给个提示 */}
            <VStack
              position="absolute"
              inset={0}
              align="center"
              justify="center"
              spacing={1}
              color="gray.500"
            >
              <Monitor size={24} opacity={0.4} />
              <Text fontSize="2xs">{showLive ? "等待预览" : "预览已关闭"}</Text>
            </VStack>
          </Flex>
        )}

        {/* 图层 */}
        {screenEnabled && renderLayer("screen", screenLayout, <Monitor size={10} />, "屏幕", true)}
        {cameraDevice !== "" && renderLayer("camera", cameraLayout, <Camera size={10} />, "摄像头", true)}
      </Box>

      {/* 画布信息 */}
      <HStack justify="space-between" fontSize="2xs" color="gray.500" px={1}>
        <Text>
          画布 {canvasW}×{canvasH}
        </Text>
        <Text>
          显示比例 {scale > 0 ? `${(scale * 100).toFixed(1)}%` : "-"}
        </Text>
      </HStack>
    </VStack>
  );
}
