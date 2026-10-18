"use client";

const GITHUB_OWNER = "bobliu0121";
const GITHUB_REPO = "neobox";

export interface ReleaseInfo {
  tag_name: string;
  name: string;
  body: string;
  assets: Array<{
    name: string;
    browser_download_url: string;
    size: number;
  }>;
  published_at: string;
  html_url: string;
}

const releaseBaseUrl = (path: string, query = "") =>
  `https://api.github.com/repos/${GITHUB_OWNER}/${GITHUB_REPO}${path}${query}`;

// GitHub Releases 公共接口允许匿名读取（未认证限流 60 次/小时），无需令牌
const requestHeaders = {
  Accept: "application/vnd.github+json",
};

export async function fetchLatestRelease(): Promise<ReleaseInfo | null> {
  try {
    const response = await fetch(releaseBaseUrl("/releases/latest"), {
      headers: requestHeaders,
    });

    if (!response.ok) {
      return null;
    }

    return (await response.json()) as ReleaseInfo;
  } catch (error) {
    return null;
  }
}

export async function fetchAllReleases(): Promise<ReleaseInfo[]> {
  try {
    const response = await fetch(releaseBaseUrl("/releases", "?per_page=100"), {
      headers: requestHeaders,
    });

    if (!response.ok) {
      return [];
    }

    return (await response.json()) as ReleaseInfo[];
  } catch (error) {
    return [];
  }
}

export async function fetchReleaseByTag(tag: string): Promise<ReleaseInfo | null> {
  try {
    const response = await fetch(
      releaseBaseUrl(`/releases/tags/${encodeURIComponent(tag)}`),
      {
        headers: requestHeaders,
      }
    );

    if (!response.ok) {
      return null;
    }

    return (await response.json()) as ReleaseInfo;
  } catch (error) {
    return null;
  }
}

export function compareVersions(current: string, latest: string): boolean {
  const cleanCurrent = current.replace(/^v/, "");
  const cleanLatest = latest.replace(/^v/, "");

  const currentParts = cleanCurrent.split(".").map(Number);
  const latestParts = cleanLatest.split(".").map(Number);

  for (let i = 0; i < Math.max(currentParts.length, latestParts.length); i++) {
    const currentPart = currentParts[i] || 0;
    const latestPart = latestParts[i] || 0;

    if (latestPart > currentPart) return true;
    if (latestPart < currentPart) return false;
  }

  return false;
}
