import { useCallback, useState } from "react";
import {
  Badge,
  Box,
  Button,
  Code,
  Heading,
  HStack,
  IconButton,
  ListItem,
  OrderedList,
  Text,
  useColorModeValue,
  useToast,
  VStack,
} from "@chakra-ui/react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import {
  ArrowLeft,
  Camera,
  Copy,
  MonitorPlay,
  Webcam,
} from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useNavPosition } from "@/components/ui/main-layout";
import { hexToRgba } from "@/lib/color-utils";

/**
 * 应用内虚拟摄像头固定设备名。
 *
 * 与后端常量严格一致：录制页摄像头下拉里显示的就是这个中文名，
 * 后端会把它插在列表最前面并带 is_virtual 标记，便于用户第一眼找到。
 * 这里不做任何本地化（设备名是标识符，不是界面文案）。
 */
const VIRTUAL_CAMERA_NAME = "NeoBox 虚拟摄像头";

/** 虚拟摄像头采集分辨率选项（决定后端 testsrc2 合成尺寸） */
const RESOLUTION_PRESETS = [
  { key: "720p", label: "1280x720" },
  { key: "1080p", label: "1920x1080" },
  { key: "5mp", label: "2592x1944" },
];

/** 说明行：左侧图标 + 标题 + 正文，正文可含代码片段 */
function InfoRow({
  icon,
  title,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
}) {
  const titleColor = useColorModeValue("gray.800", "#ffffff");
  const bodyColor = useColorModeValue("gray.500", "#a0a0a0");
  return (
    <HStack align="start" spacing={3}>
      <Box flexShrink={0} mt={0.5}>
        {icon}
      </Box>
      <Box flex={1} minW={0}>
        <Text fontSize="sm" fontWeight="semibold" color={titleColor}>
          {title}
        </Text>
        <Text fontSize="sm" color={bodyColor} mt={1} lineHeight="tall">
          {children}
        </Text>
      </Box>
    </HStack>
  );
}

export default function VirtualCameraPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useToast();
  const { config: themeConfig, getContrastTextColor } = useThemeColor();
  const adaptiveTitle = useAdaptiveTextColor();
  const navPosition = useNavPosition();

  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);

  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#888888");
  const codeBg = useColorModeValue("gray.100", "rgba(255,255,255,0.08)");
  const codeColor = useColorModeValue("gray.800", "#f0f0f0");
  const tipBg = useColorModeValue("orange.50", "rgba(255,165,0,0.10)");
  const tipBorder = useColorModeValue("orange.200", "rgba(255,165,0,0.30)");
  const tipText = useColorModeValue("orange.800", "orange.200");

  const contrastText = getContrastTextColor();
  const fillMinH = navPosition === "top" ? "calc(100vh - 152px)" : "calc(100vh - 88px)";

  const [copied, setCopied] = useState(false);

  const copyDeviceName = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(VIRTUAL_CAMERA_NAME);
      setCopied(true);
      toast({
        title: t("virtualCamera.copied"),
        status: "success",
        duration: 1500,
        isClosable: true,
      });
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("Failed to copy virtual camera name:", e);
      toast({
        title: t("virtualCamera.copyFailed"),
        description: String(e),
        status: "error",
        duration: 2500,
        isClosable: true,
      });
    }
  }, [toast, t]);

  return (
    <Box pt={2} minH={fillMinH} display="flex" flexDirection="column">
      <VStack align="stretch" spacing={4} flex={1}>
        {/* 标题区：返回 + 图标 + 标题，与其它内置工具页保持一致 */}
        <HStack spacing={3} flexWrap="wrap">
          <IconButton
            aria-label={t("virtualCamera.back")}
            icon={<ArrowLeft size={20} />}
            variant="ghost"
            onClick={() => navigate("/builtin-tools")}
          />
          <HStack spacing={2}>
            <Webcam size={22} color={themeColorHex} />
            <Heading size="lg" color={adaptiveTitle.text} textShadow={adaptiveTitle.shadow}>
              {t("virtualCamera.title")}
            </Heading>
          </HStack>
          <Badge fontSize="2xs" colorScheme="teal" borderRadius="full" px={2}>
            {t("virtualCamera.noDriverBadge")}
          </Badge>
        </HStack>

        <Text fontSize="sm" color={subTextColor}>
          {t("virtualCamera.desc")}
        </Text>

        {/* 主操作卡：设备名 + 打开录制页 */}
        <LiquidGlassCard p={5}>
          <VStack align="stretch" spacing={4}>
            <HStack justify="space-between" align="start" flexWrap="wrap" gap={3}>
              <VStack align="start" spacing={1} minW={0}>
                <Text fontSize="sm" fontWeight="semibold" color={headingColor}>
                  {t("virtualCamera.deviceNameLabel")}
                </Text>
                <Text fontSize="xs" color={subTextColor}>
                  {t("virtualCamera.deviceNameHint")}
                </Text>
              </VStack>
              <Button
                size="sm"
                onClick={copyDeviceName}
                leftIcon={<Copy size={14} />}
                variant="outline"
                borderColor={themeColorRgba(0.4)}
                color={themeColorHex}
                _hover={{ bg: themeColorRgba(0.1) }}
                flexShrink={0}
              >
                {copied ? t("virtualCamera.copied") : t("virtualCamera.copy")}
              </Button>
            </HStack>

            {/* 固定设备名：等宽代码样式展示，保证与下拉项逐字一致 */}
            <Code
              display="block"
              w="full"
              px={4}
              py={3}
              borderRadius="lg"
              fontSize="md"
              fontFamily="mono"
              bg={codeBg}
              color={codeColor}
              whiteSpace="pre-wrap"
              wordBreak="break-all"
            >
              {VIRTUAL_CAMERA_NAME}
            </Code>

            <HStack spacing={3} flexWrap="wrap">
              <Button
                size="md"
                leftIcon={<Camera size={16} />}
                bg={themeColorHex}
                color={contrastText}
                _hover={{ bg: themeColorRgba(0.85) }}
                _active={{ transform: "scale(0.97)" }}
                onClick={() => navigate("/recording")}
              >
                {t("virtualCamera.openRecording")}
              </Button>
              <Text fontSize="xs" color={subTextColor}>
                {t("virtualCamera.openRecordingHint")}
              </Text>
            </HStack>
          </VStack>
        </LiquidGlassCard>

        {/* 使用步骤 */}
        <LiquidGlassCard p={5}>
          <VStack align="stretch" spacing={3}>
            <HStack spacing={2}>
              <MonitorPlay size={18} color={themeColorHex} />
              <Text fontSize="sm" fontWeight="semibold" color={headingColor}>
                {t("virtualCamera.stepsTitle")}
              </Text>
            </HStack>
            <OrderedList spacing={2} fontSize="sm" color={subTextColor} pl={2}>
              <ListItem>
                {t("virtualCamera.step1")}
              </ListItem>
              <ListItem>
                {t("virtualCamera.step2Before")}
                <Code mx={1} fontSize="xs" fontFamily="mono" bg={codeBg} color={themeColorHex}>
                  {VIRTUAL_CAMERA_NAME}
                </Code>
                {t("virtualCamera.step2After")}
              </ListItem>
              <ListItem>
                {t("virtualCamera.step3")}
              </ListItem>
            </OrderedList>
            <Text fontSize="xs" color={subTextColor}>
              {t("virtualCamera.stepHint")}
            </Text>
          </VStack>
        </LiquidGlassCard>

        {/* 排查价值：A/B 对照判定瓶颈位置 */}
        <LiquidGlassCard p={5}>
          <VStack align="stretch" spacing={3}>
            <Text fontSize="sm" fontWeight="semibold" color={headingColor}>
              {t("virtualCamera.diagnoseTitle")}
            </Text>
            <Text fontSize="sm" color={subTextColor} lineHeight="tall">
              {t("virtualCamera.diagnoseDesc")}
            </Text>
            <VStack align="stretch" spacing={2} pl={1}>
              <InfoRow icon={<Box w={1.5} h={1.5} borderRadius="full" bg="green.400" mt={2} />} title={t("virtualCamera.caseACondition")}>
                {t("virtualCamera.caseAConclusion")}
              </InfoRow>
              <InfoRow icon={<Box w={1.5} h={1.5} borderRadius="full" bg="orange.400" mt={2} />} title={t("virtualCamera.caseBCondition")}>
                {t("virtualCamera.caseBConclusion")}
              </InfoRow>
            </VStack>
          </VStack>
        </LiquidGlassCard>

        {/* 模拟大分辨率采集 */}
        <LiquidGlassCard p={5}>
          <VStack align="stretch" spacing={3}>
            <Text fontSize="sm" fontWeight="semibold" color={headingColor}>
              {t("virtualCamera.resolutionTitle")}
            </Text>
            <Text fontSize="sm" color={subTextColor} lineHeight="tall">
              {t("virtualCamera.resolutionDesc")}
            </Text>
            <HStack spacing={2} flexWrap="wrap">
              {RESOLUTION_PRESETS.map((preset) => (
                <Badge
                  key={preset.key}
                  fontFamily="mono"
                  fontSize="xs"
                  px={2.5}
                  py={1}
                  borderRadius="md"
                  colorScheme={preset.key === "5mp" ? "orange" : "gray"}
                  variant="subtle"
                >
                  {preset.label}
                </Badge>
              ))}
            </HStack>
            <Text fontSize="xs" color={subTextColor}>
              {t("virtualCamera.resolutionNote")}
            </Text>
            <Box p={3} borderRadius="md" bg={tipBg} border="1px solid" borderColor={tipBorder}>
              <Text fontSize="xs" color={tipText} lineHeight="tall">
                {t("virtualCamera.resolutionTip")}
              </Text>
            </Box>
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
