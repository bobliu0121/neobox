import "./lib/browser-mock";
import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { ChakraProvider, ColorModeScript, Spinner, Center } from "@chakra-ui/react";
import { CacheProvider } from "@emotion/react";
import createCache from "@emotion/cache";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "./index.css";
import { I18nextProvider } from "react-i18next";
import i18n from "./lib/i18n";
import { BackgroundProvider } from "./contexts/background-context";
import { ThemeColorProvider } from "./contexts/theme-color-context";
import { AppStartupProvider } from "./contexts/app-startup-context";
import { UpdateProvider } from "./contexts/update-context";
import { FontProvider } from "./contexts/font-context";
import { ThemeModeProvider } from "./contexts/theme-mode-context";
import { LiquidGlassSvgFilter } from "./components/special/liquid-glass-svg-filter";
import theme from "./lib/theme";
const emotionCache = createCache({
  key: "css",
  prepend: true,
});

function Root() {
  const [isI18nReady, setIsI18nReady] = useState(false);

  useEffect(() => {
    if (i18n.isInitialized) {
      setIsI18nReady(true);
    } else {
      i18n.on("initialized", () => setIsI18nReady(true));
    }
  }, []);

  if (!isI18nReady) {
    return (
      <Center h="100vh">
        <Spinner size="xl" />
      </Center>
    );
  }

  return (
    <React.StrictMode>
      <CacheProvider value={emotionCache}>
        <ColorModeScript initialColorMode={theme.config.initialColorMode} />
        <I18nextProvider i18n={i18n}>
          <ChakraProvider theme={theme}>
            <BrowserRouter>
              <AppStartupProvider>
                <UpdateProvider>
                  <BackgroundProvider>
                    <ThemeColorProvider>
                      <FontProvider>
                        <LiquidGlassSvgFilter />
                        <ThemeModeProvider>
                          <App />
                        </ThemeModeProvider>
                      </FontProvider>
                    </ThemeColorProvider>
                  </BackgroundProvider>
                </UpdateProvider>
              </AppStartupProvider>
            </BrowserRouter>
          </ChakraProvider>
        </I18nextProvider>
      </CacheProvider>
    </React.StrictMode>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
