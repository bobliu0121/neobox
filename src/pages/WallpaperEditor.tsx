import { useCallback, useEffect, useRef, useState } from "react";
import {
  Modal,
  ModalOverlay,
  ModalContent,
  ModalHeader,
  ModalBody,
  ModalFooter,
  Button,
  HStack,
  VStack,
  Slider,
  SliderTrack,
  SliderFilledTrack,
  SliderThumb,
  Text,
  Box,
  useToast,
  useColorModeValue,
} from "@chakra-ui/react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { RotateCw, RotateCcw, FlipHorizontal2, FlipVertical2, Save, X, Crop } from "lucide-react";

interface WallpaperEditorProps {
  path: string;
  onClose: () => void;
  onSaved: (newPath?: string) => void;
}

const MAX_DIM = 2560;

export default function WallpaperEditor({ path, onClose, onSaved }: WallpaperEditorProps) {
  const { t } = useTranslation();
  const toast = useToast();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const cropBoxRef = useRef<HTMLDivElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [loading, setLoading] = useState(true);
  const [rotation, setRotation] = useState(0);
  const [flipH, setFlipH] = useState(false);
  const [flipV, setFlipV] = useState(false);
  const [brightness, setBrightness] = useState(100);
  const [contrast, setContrast] = useState(100);
  const [saving, setSaving] = useState(false);
  const [cropMode, setCropMode] = useState(false);
  // 裁剪框位置和大小（相对容器百分比 0-100）
  const [cropRect, setCropRect] = useState<{ x: number; y: number; w: number; h: number }>({
    x: 10, y: 10, w: 80, h: 80,
  });
  const dragRef = useRef<{ mode: string; startX: number; startY: number; orig: typeof cropRect } | null>(null);
  const bg = useColorModeValue("gray.800", "#000000");

  // 加载图片
  useEffect(() => {
    const img = new Image();
    img.onload = () => {
      imgRef.current = img;
      // 默认裁剪框居中
      setCropRect({ x: 10, y: 10, w: 80, h: 80 });
      setLoading(false);
    };
    img.onerror = () => {
      toast({ title: t("wallpaper.editorLoadFailed"), status: "error", duration: 3000 });
      onClose();
    };
    img.src = convertFileSrc(path);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  // 绘制画布
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const img = imgRef.current;
    if (!canvas || !img) return;
    const rotated = rotation % 2 === 1;
    const baseW = img.naturalWidth;
    const baseH = img.naturalHeight;
    const scale = Math.min(1, MAX_DIM / Math.max(baseW, baseH));
    const w = rotated ? baseH * scale : baseW * scale;
    const h = rotated ? baseW * scale : baseH * scale;
    canvas.width = Math.max(1, Math.round(w));
    canvas.height = Math.max(1, Math.round(h));
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.filter = `brightness(${brightness / 100}) contrast(${contrast / 100})`;
    ctx.save();
    ctx.translate(canvas.width / 2, canvas.height / 2);
    ctx.rotate((rotation * Math.PI) / 2);
    if (flipH) ctx.scale(-1, 1);
    if (flipV) ctx.scale(1, -1);
    ctx.drawImage(img, (-baseW * scale) / 2, (-baseH * scale) / 2, baseW * scale, baseH * scale);
    ctx.restore();
  }, [rotation, flipH, flipV, brightness, contrast]);

  useEffect(() => {
    if (!loading) draw();
  }, [draw, loading]);

  // 裁剪框鼠标交互
  const onCropMouseDown = (e: React.MouseEvent, mode: string) => {
    e.preventDefault();
    e.stopPropagation();
    dragRef.current = {
      mode,
      startX: e.clientX,
      startY: e.clientY,
      orig: { ...cropRect },
    };
  };

  useEffect(() => {
    const onMouseMove = (e: MouseEvent) => {
      const drag = dragRef.current;
      const container = containerRef.current;
      if (!drag || !container) return;
      const rect = container.getBoundingClientRect();
      const dxPct = ((e.clientX - drag.startX) / rect.width) * 100;
      const dyPct = ((e.clientY - drag.startY) / rect.height) * 100;
      const { orig } = drag;

      if (drag.mode === "move") {
        let nx = orig.x + dxPct;
        let ny = orig.y + dyPct;
        nx = Math.max(0, Math.min(100 - orig.w, nx));
        ny = Math.max(0, Math.min(100 - orig.h, ny));
        setCropRect({ ...orig, x: nx, y: ny });
      } else if (drag.mode === "resize-br") {
        let nw = orig.w + dxPct;
        let nh = orig.h + dyPct;
        nw = Math.max(5, Math.min(100 - orig.x, nw));
        nh = Math.max(5, Math.min(100 - orig.y, nh));
        setCropRect({ ...orig, w: nw, h: nh });
      }
    };
    const onMouseUp = () => {
      dragRef.current = null;
    };
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, []);

  const handleSave = useCallback(async () => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    setSaving(true);
    try {
      const ext = path.split(".").pop()?.toLowerCase() || "";
      let savePath = path;
      let mime = "image/png";
      if (ext === "jpg" || ext === "jpeg") mime = "image/jpeg";
      else if (ext === "webp") mime = "image/webp";
      else if (ext === "png") mime = "image/png";
      else {
        savePath = path.replace(/\.[^.]+$/, "") + "_edited.png";
        mime = "image/png";
      }

      // 裁剪模式：在离屏 canvas 上裁剪
      let outputCanvas = canvas;
      if (cropMode) {
        const sx = (cropRect.x / 100) * canvas.width;
        const sy = (cropRect.y / 100) * canvas.height;
        const sw = (cropRect.w / 100) * canvas.width;
        const sh = (cropRect.h / 100) * canvas.height;
        if (sw > 1 && sh > 1) {
          const cropped = document.createElement("canvas");
          cropped.width = Math.round(sw);
          cropped.height = Math.round(sh);
          const cctx = cropped.getContext("2d");
          if (cctx) {
            cctx.drawImage(canvas, sx, sy, sw, sh, 0, 0, sw, sh);
            outputCanvas = cropped;
          }
        }
      }

      const dataUrl = outputCanvas.toDataURL(mime, 0.92);
      await invoke("save_wallpaper_edit", { path: savePath, dataUrl });
      toast({
        title: savePath === path ? t("wallpaper.editorSaved") : t("wallpaper.editorSavedAs"),
        status: "success",
        duration: 2500,
      });
      onSaved(savePath === path ? undefined : savePath);
      onClose();
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    } finally {
      setSaving(false);
    }
  }, [path, toast, t, onSaved, onClose, cropMode, cropRect]);

  return (
    <Modal isOpen onClose={onClose} size="4xl">
      <ModalOverlay />
      <ModalContent>
        <ModalHeader>{t("wallpaper.editorTitle")}</ModalHeader>
        <ModalBody>
          <VStack spacing={4}>
            <Box
              ref={containerRef}
              position="relative"
              width="100%"
              maxH="52vh"
              overflow="hidden"
              bg={bg}
              borderRadius="md"
              display="flex"
              alignItems="center"
              justifyContent="center"
            >
              <canvas ref={canvasRef} style={{ maxWidth: "100%", maxHeight: "52vh", objectFit: "contain" }} />
              {cropMode && (
                <>
                  {/* 遮罩暗化外部区域 */}
                  <Box position="absolute" top={0} left={0} right={0} height={`${cropRect.y}%`} bg="blackAlpha.60%" pointerEvents="none" />
                  <Box position="absolute" bottom={0} left={0} right={0} height={`${100 - cropRect.y - cropRect.h}%`} bg="blackAlpha.60%" pointerEvents="none" />
                  <Box position="absolute" top={`${cropRect.y}%`} left={0} width={`${cropRect.x}%`} height={`${cropRect.h}%`} bg="blackAlpha.60%" pointerEvents="none" />
                  <Box position="absolute" top={`${cropRect.y}%`} left={`${cropRect.x + cropRect.w}%`} width={`${100 - cropRect.x - cropRect.w}%`} height={`${cropRect.h}%`} bg="blackAlpha.60%" pointerEvents="none" />
                  {/* 裁剪框 */}
                  <Box
                    ref={cropBoxRef}
                    position="absolute"
                    top={`${cropRect.y}%`}
                    left={`${cropRect.x}%`}
                    width={`${cropRect.w}%`}
                    height={`${cropRect.h}%`}
                    border="2px solid white"
                    boxShadow="0 0 0 1px rgba(0,0,0,0.3)"
                    cursor="move"
                    onMouseDown={(e) => onCropMouseDown(e, "move")}
                  >
                    {/* 右下角调整手柄 */}
                    <Box
                      position="absolute"
                      bottom="-6px"
                      right="-6px"
                      width="12px"
                      height="12px"
                      bg="white"
                      border="1px solid gray"
                      cursor="nwse-resize"
                      onMouseDown={(e) => onCropMouseDown(e, "resize-br")}
                    />
                  </Box>
                </>
              )}
            </Box>
            <HStack spacing={2} flexWrap="wrap">
              <Button size="sm" leftIcon={<RotateCcw />} onClick={() => setRotation((r) => (r + 3) % 4)}>
                {t("wallpaper.editorRotateLeft")}
              </Button>
              <Button size="sm" leftIcon={<RotateCw />} onClick={() => setRotation((r) => (r + 1) % 4)}>
                {t("wallpaper.editorRotateRight")}
              </Button>
              <Button size="sm" leftIcon={<FlipHorizontal2 />} onClick={() => setFlipH((v) => !v)}>
                {t("wallpaper.editorFlipH")}
              </Button>
              <Button size="sm" leftIcon={<FlipVertical2 />} onClick={() => setFlipV((v) => !v)}>
                {t("wallpaper.editorFlipV")}
              </Button>
              <Button
                size="sm"
                leftIcon={<Crop />}
                colorScheme={cropMode ? "blue" : "gray"}
                variant={cropMode ? "solid" : "outline"}
                onClick={() => setCropMode((v) => !v)}
              >
                {t("wallpaper.editorCrop") || "裁剪"}
              </Button>
            </HStack>
            <VStack spacing={2} width="100%">
              <HStack width="100%" spacing={3}>
                <Text fontSize="xs" color="gray.500" width="90px">
                  {t("wallpaper.editorBrightness")}
                </Text>
                <Slider value={brightness} min={20} max={200} onChange={setBrightness} flex="1">
                  <SliderTrack><SliderFilledTrack /></SliderTrack>
                  <SliderThumb />
                </Slider>
                <Text fontSize="xs" width="36px" textAlign="right">
                  {brightness}%
                </Text>
              </HStack>
              <HStack width="100%" spacing={3}>
                <Text fontSize="xs" color="gray.500" width="90px">
                  {t("wallpaper.editorContrast")}
                </Text>
                <Slider value={contrast} min={20} max={200} onChange={setContrast} flex="1">
                  <SliderTrack><SliderFilledTrack /></SliderTrack>
                  <SliderThumb />
                </Slider>
                <Text fontSize="xs" width="36px" textAlign="right">
                  {contrast}%
                </Text>
              </HStack>
            </VStack>
          </VStack>
        </ModalBody>
        <ModalFooter>
          <Button size="sm" leftIcon={<X />} variant="ghost" onClick={onClose} mr={2}>
            {t("wallpaper.editorCancel")}
          </Button>
          <Button size="sm" leftIcon={<Save />} colorScheme="blue" onClick={handleSave} isLoading={saving}>
            {t("wallpaper.editorSave")}
          </Button>
        </ModalFooter>
      </ModalContent>
    </Modal>
  );
}
