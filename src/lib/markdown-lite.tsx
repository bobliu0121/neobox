import { Box, HStack, Text } from "@chakra-ui/react";
import type { ReactNode } from "react";

/**
 * 轻量 Markdown 渲染：仅处理标题、列表、代码块、粗体/行内代码/链接，
 * 供「关于 → 更新日志」与「主页 → Release 历史」共用，避免引入额外依赖。
 */
export function renderMarkdownInline(text: string): ReactNode[] {
  const parts = text
    .split(/(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)]+\))/g)
    .filter(Boolean);
  return parts.map((part, index) => {
    if (part.startsWith("**") && part.endsWith("**")) {
      return (
        <strong key={index} style={{ fontWeight: 700 }}>
          {part.slice(2, -2)}
        </strong>
      );
    }
    if (part.startsWith("`") && part.endsWith("`")) {
      return (
        <code
          key={index}
          style={{
            fontFamily: "Consolas, monospace",
            background: "rgba(127,127,127,0.18)",
            padding: "1px 4px",
            borderRadius: 4,
          }}
        >
          {part.slice(1, -1)}
        </code>
      );
    }
    const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(part);
    if (link) {
      return <span key={index}>{link[1]}</span>;
    }
    return <span key={index}>{part}</span>;
  });
}

export function renderMarkdownLite(
  body: string,
  textColor: string,
  bulletColor: string,
): ReactNode[] {
  const nodes: ReactNode[] = [];
  const codeLines: string[] = [];
  let inCode = false;

  const flushCode = () => {
    if (!codeLines.length) return;
    nodes.push(
      <Box
        key={`code-${nodes.length}`}
        mt={1}
        mb={2}
        p={3}
        borderRadius="md"
        bg="rgba(127,127,127,0.12)"
        overflowX="auto"
      >
        <Text as="pre" fontSize="xs" fontFamily="Consolas, monospace" color={textColor} m={0}>
          {codeLines.join("\n")}
        </Text>
      </Box>,
    );
    codeLines.length = 0;
  };

  body.split(/\r?\n/).forEach((line, index) => {
    const trimmed = line.trim();
    if (trimmed.startsWith("```")) {
      if (inCode) {
        flushCode();
        inCode = false;
      } else {
        inCode = true;
      }
      return;
    }
    if (inCode) {
      codeLines.push(line);
      return;
    }
    if (!trimmed) {
      nodes.push(<Box key={`sp-${index}`} h="6px" />);
      return;
    }
    const heading = /^(#{1,4})\s+(.*)$/.exec(trimmed);
    if (heading) {
      nodes.push(
        <Text
          key={index}
          fontWeight="bold"
          fontSize={heading[1].length <= 2 ? "md" : "sm"}
          color={textColor}
          mt={index === 0 ? 0 : 3}
          mb={1}
        >
          {renderMarkdownInline(heading[2])}
        </Text>,
      );
      return;
    }
    const bullet = /^[-*]\s+(.*)$/.exec(trimmed);
    if (bullet) {
      nodes.push(
        <HStack key={index} align="start" spacing={2} mb={1}>
          <Text color={bulletColor} fontSize="sm" lineHeight="1.6">
            •
          </Text>
          <Text color={textColor} fontSize="sm" lineHeight="1.6" flex={1}>
            {renderMarkdownInline(bullet[1])}
          </Text>
        </HStack>,
      );
      return;
    }
    nodes.push(
      <Text key={index} color={textColor} fontSize="sm" lineHeight="1.6" whiteSpace="pre-wrap">
        {renderMarkdownInline(trimmed)}
      </Text>,
    );
  });

  if (inCode) flushCode();
  return nodes;
}
