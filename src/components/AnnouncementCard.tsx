import {
  Box,
  Text,
  VStack,
  useColorModeValue,
  Modal,
  ModalOverlay,
  ModalContent,
  ModalHeader,
  ModalBody,
  ModalCloseButton,
  useDisclosure,
  Badge,
  Divider,
  HStack,
  Spinner,
  Button,
} from "@chakra-ui/react";
import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ExternalLink, GitBranch } from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useBackground } from "@/contexts/background-context";
import { store } from "@/lib/store";
import { renderMarkdownLite } from "@/lib/markdown-lite";
import { openExternal } from "@/hooks/use-ads";

/** 后端 `get_release_history` 返回的 Release 条目 */
interface ReleaseInfo {
  tag: string;
  name: string;
  body: string;
  published_at: string;
  html_url: string;
  prerelease: boolean;
}

function formatReleaseDate(value: string): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

export function useAnnouncementEnabled() {
  const [state, setState] = useState({ enabled: false, ready: false });

  useEffect(() => {
    (async () => {
      let enabled = true;
      const saved = await store.get<boolean>("neobox_announcement_enabled");
      if (saved !== null && saved !== undefined) {
        enabled = saved;
      } else {
        const ls = localStorage.getItem("neobox_announcement_enabled");
        if (ls !== null) enabled = ls === "true";
      }
      setState((s) => ({ ...s, enabled, ready: true }));
    })();
  }, []);

  useEffect(() => {
    const handler = (e: CustomEvent) => setState((s) => ({ ...s, enabled: e.detail }));
    window.addEventListener("announcement-setting-changed", handler as EventListener);
    return () => window.removeEventListener("announcement-setting-changed", handler as EventListener);
  }, []);

  return state;
}

export function AnnouncementCard() {
  const { t } = useTranslation();
  const { liquidGlassEnabled } = useBackground();
  const { isOpen, onOpen, onClose } = useDisclosure();
  const [releases, setReleases] = useState<ReleaseInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasUnread, setHasUnread] = useState(false);

  const labelColor = useColorModeValue("gray.500", "#ffffff");
  const cardBg = useColorModeValue("white", "#1a1a1a");
  const borderColor = useColorModeValue("gray.200", "#333333");
  const contentColor = useColorModeValue("gray.700", "#cccccc");
  const dateColor = useColorModeValue("gray.400", "#666666");
  const countColor = useColorModeValue("purple.500", "#b794f4");
  const modalBg = useColorModeValue("white", "#1a1a1a");
  const linkColor = useColorModeValue("purple.500", "#b794f4");

  useEffect(() => {
    const fetchReleases = async () => {
      try {
        const list = await invoke<ReleaseInfo[]>("get_release_history");
        setReleases(list);
        let lastReadTime = await store.get<string>("neobox_announcement_last_read");
        if (!lastReadTime) {
          lastReadTime = localStorage.getItem("neobox_announcement_last_read");
        }
        if (list.length > 0 && lastReadTime) {
          const lastRead = new Date(lastReadTime);
          setHasUnread(list.some((release) => new Date(release.published_at) > lastRead));
        } else if (list.length > 0) {
          setHasUnread(true);
        }
      } catch (e) {
        console.error("Failed to fetch release history:", e);
      }
    };
    fetchReleases();
  }, []);

  const handleOpenModal = async () => {
    setLoading(true);
    onOpen();
    try {
      const list = await invoke<ReleaseInfo[]>("get_release_history");
      setReleases(list);
      const now = new Date().toISOString();
      localStorage.setItem("neobox_announcement_last_read", now);
      await store.set("neobox_announcement_last_read", now);
      await store.save();
      setHasUnread(false);
    } catch (e) {
      console.error("Failed to fetch release history:", e);
    } finally {
      setLoading(false);
    }
  };

  const cardContent = (
    <VStack spacing={0} align="center">
      <Text fontSize="2xs" color={labelColor}>
        {t("home.announcement")}
      </Text>
      <Box cursor="pointer" userSelect="none" position="relative">
        <Text
          fontSize="2xl"
          fontWeight="bold"
          color={hasUnread ? countColor : labelColor}
          transition="color 0.3s"
        >
          {releases.length}
        </Text>
        {hasUnread && (
          <Box position="absolute" top="-2px" right="-8px" w="8px" h="8px" bg="red.500" borderRadius="full" />
        )}
      </Box>
    </VStack>
  );

  const modalContent = (
    <Modal isOpen={isOpen} onClose={onClose} size="lg" scrollBehavior="inside">
      <ModalOverlay />
      <ModalContent maxH="80vh" bg={modalBg}>
        <ModalHeader>
          <HStack spacing={2}>
            <GitBranch size={18} />
            <Text>{t("home.releaseHistory")}</Text>
          </HStack>
        </ModalHeader>
        <ModalCloseButton />
        <ModalBody
          pb={6}
          overflowY="auto"
          sx={{
            "&::-webkit-scrollbar": { width: "6px" },
            "&::-webkit-scrollbar-track": { background: "transparent" },
            "&::-webkit-scrollbar-thumb": {
              background: useColorModeValue("gray.300", "gray.600"),
              borderRadius: "3px",
            },
          }}
        >
          {loading && releases.length === 0 ? (
            <VStack py={8}>
              <Spinner size="lg" color={countColor} />
              <Text color={labelColor} fontSize="sm">
                {t("home.loading")}
              </Text>
            </VStack>
          ) : releases.length === 0 ? (
            <VStack py={8}>
              <Text color={labelColor}>{t("home.noAnnouncement")}</Text>
            </VStack>
          ) : (
            <VStack spacing={4} align="stretch">
              {releases.map((release) => (
                <Box key={release.tag}>
                  <VStack align="stretch" spacing={2}>
                    <HStack justify="space-between" align="start" flexWrap="wrap" gap={2}>
                      <HStack flexWrap="wrap">
                        <Badge colorScheme="purple" fontSize="xs">
                          {release.tag}
                        </Badge>
                        <Text fontWeight="bold" fontSize="md" color={contentColor}>
                          {release.name}
                        </Text>
                        {release.prerelease && (
                          <Badge colorScheme="orange" fontSize="2xs">
                            Pre-release
                          </Badge>
                        )}
                      </HStack>
                      <Text fontSize="xs" color={dateColor}>
                        {formatReleaseDate(release.published_at)}
                      </Text>
                    </HStack>
                    {release.body?.trim() ? (
                      <Box>{renderMarkdownLite(release.body, contentColor, countColor)}</Box>
                    ) : (
                      <Text fontSize="sm" color={dateColor}>
                        {t("home.releaseNoNotes")}
                      </Text>
                    )}
                    {release.html_url && (
                      <Button
                        size="xs"
                        variant="link"
                        color={linkColor}
                        alignSelf="flex-start"
                        rightIcon={<ExternalLink size={12} />}
                        onClick={() => void openExternal(release.html_url)}
                      >
                        {t("home.viewOnGitHub")}
                      </Button>
                    )}
                  </VStack>
                  <Divider mt={3} />
                </Box>
              ))}
            </VStack>
          )}
        </ModalBody>
      </ModalContent>
    </Modal>
  );

  return (
    <>
      {liquidGlassEnabled ? (
        <LiquidGlassCard py={2} px={3} w="90px" onClick={handleOpenModal}>
          {cardContent}
        </LiquidGlassCard>
      ) : (
        <Box
          bg={cardBg}
          borderRadius="xl"
          border="1px solid"
          borderColor={borderColor}
          py={2}
          px={3}
          w="90px"
          onClick={handleOpenModal}
          cursor="pointer"
          _hover={{ borderColor: countColor }}
          transition="border-color 0.2s"
        >
          {cardContent}
        </Box>
      )}
      {modalContent}
    </>
  );
}
