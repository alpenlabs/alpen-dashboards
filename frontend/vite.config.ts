import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
    plugins: [react()],
    optimizeDeps: {
        include: ["@tanstack/react-query"],
    },
    server: {
        allowedHosts: [
            "status.testnet.alpenlabs.io",
            "status.testnet-staging.stratabtc.org",
            "status.development.stratabtc.org",
        ],
    },
});
