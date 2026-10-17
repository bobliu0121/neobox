import { Box, Text, VStack, useColorModeValue } from "@chakra-ui/react";
import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useBackground } from "@/contexts/background-context";
import { store } from "@/lib/store";

const quotes = [
  { text: "千里之行，始于足下。", author: "佚名" },
  { text: "学而不思则罔，思而不学则殆。", author: "佚名" },
  { text: "不积跬步，无以至千里。", author: "佚名" },
  { text: "宝剑锋从磨砺出，梅花香自苦寒来。", author: "佚名" },
  { text: "山重水复疑无路，柳暗花明又一村。", author: "佚名" },
  { text: "路漫漫其修远兮，吾将上下而求索。", author: "佚名" },
  { text: "纸上得来终觉浅，绝知此事要躬行。", author: "佚名" },
  { text: "三人行，必有我师焉。", author: "佚名" },
  { text: "天行健，君子以自强不息。", author: "佚名" },
  { text: "博观而约取，厚积而薄发。", author: "佚名" },
  { text: "长风破浪会有时，直挂云帆济沧海。", author: "佚名" },
  { text: "会当凌绝顶，一览众山小。", author: "佚名" },
  { text: "业精于勤，荒于嬉。", author: "佚名" },
  { text: "一寸光阴一寸金，寸金难买寸光阴。", author: "佚名" },
  { text: "少壮不努力，老大徒伤悲。", author: "佚名" },
  { text: "水滴石穿，绳锯木断。", author: "佚名" },
  { text: "玉不琢，不成器。", author: "佚名" },
  { text: "知人者智，自知者明。", author: "佚名" },
  { text: "天将降大任于是人也，必先苦其心志。", author: "佚名" },
  { text: "锲而不舍，金石可镂。", author: "佚名" },
];

function getRandomQuote() {
  const index = Math.floor(Math.random() * quotes.length);
  return quotes[index];
}

export function useRandomQuoteEnabled() {
  const [state, setState] = useState({ enabled: false, ready: false });

  useEffect(() => {
    (async () => {
      let enabled = true;
      const saved = await store.get<boolean>("neobox_random_quote_enabled");
      if (saved !== null && saved !== undefined) {
        enabled = saved;
      } else {
        const ls = localStorage.getItem("neobox_random_quote_enabled");
        if (ls !== null) enabled = ls === "true";
      }
      setState((s) => ({ ...s, enabled, ready: true }));
    })();
  }, []);

  useEffect(() => {
    const handler = (e: CustomEvent) => setState((s) => ({ ...s, enabled: e.detail }));
    window.addEventListener("random-quote-setting-changed", handler as EventListener);
    return () => window.removeEventListener("random-quote-setting-changed", handler as EventListener);
  }, []);

  return state;
}

export function RandomQuote() {
  const { t } = useTranslation();
  const { liquidGlassEnabled } = useBackground();

  const [quote, setQuote] = useState(() => getRandomQuote());

  const labelColor = useColorModeValue("gray.500", "#ffffff");
  const textColor = useColorModeValue("gray.700", "#ffffff");
  const authorColor = useColorModeValue("gray.400", "#888888");
  const cardBg = useColorModeValue("white", "#1a1a1a");
  const borderColor = useColorModeValue("gray.200", "#333333");
  const hoverBorderColor = useColorModeValue("purple.500", "#b794f4");

  const handleRefresh = () => {
    setQuote(getRandomQuote());
  };

  const cardContent = (
    <VStack spacing={0.5} align="flex-start" minW="90px">
      <Text fontSize="2xs" color={labelColor}>
        {t("home.randomQuote")}
      </Text>
      <Box cursor="pointer" onClick={handleRefresh} userSelect="none">
        <Text fontSize="sm" color={textColor} fontWeight="medium" whiteSpace="nowrap">
          {quote.text}
        </Text>
        <Text fontSize="2xs" color={authorColor} textAlign="right">
          -{quote.author}
        </Text>
      </Box>
    </VStack>
  );

  if (liquidGlassEnabled) {
    return (
      <LiquidGlassCard py={2} px={3} minW="90px">
        {cardContent}
      </LiquidGlassCard>
    );
  }

  return (
    <Box
      bg={cardBg}
      borderRadius="xl"
      border="1px solid"
      borderColor={borderColor}
      py={2}
      px={3}
      minW="90px"
      cursor="pointer"
      _hover={{ borderColor: hoverBorderColor }}
      transition="border-color 0.2s"
    >
      {cardContent}
    </Box>
  );
}
