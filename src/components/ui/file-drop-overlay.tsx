import { Box, HStack, Text } from "@chakra-ui/react";
import { FileDown } from "lucide-react";

/** 文件拖入时的全屏提示层（固定定位，不拦截目标页面的其它操作） */
export function FileDropOverlay({ visible, hint = "松开即可导入文件" }: { visible: boolean; hint?: string }) {
  if (!visible) return null;
  return (
    <Box
      position="fixed"
      inset={0}
      zIndex={9999}
      display="flex"
      alignItems="center"
      justifyContent="center"
      bg="rgba(10,10,12,0.55)"
      sx={{ backdropFilter: "blur(6px)", WebkitBackdropFilter: "blur(6px)", pointerEvents: "none" }}
    >
      <Box
        px={8}
        py={6}
        borderRadius="2xl"
        border="2px dashed rgba(255,255,255,0.55)"
        bg="rgba(20,20,24,0.65)"
        color="#fff"
        boxShadow="0 20px 60px rgba(0,0,0,0.45)"
      >
        <HStack spacing={3}>
          <FileDown size={30} />
          <Text fontSize="lg" fontWeight="bold">
            {hint}
          </Text>
        </HStack>
      </Box>
    </Box>
  );
}
