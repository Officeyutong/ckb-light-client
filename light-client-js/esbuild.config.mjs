import * as esbuild from 'esbuild'
import inlineWorkerPlugin from 'esbuild-plugin-inline-worker';
import { polyfillNode } from "esbuild-plugin-polyfill-node";
const profile = process.env.PROFILE;
await esbuild.build({
    entryPoints: ["./src/index.ts"],
    bundle: true,
    outdir: "dist",
    plugins: [polyfillNode(), inlineWorkerPlugin({
    })],
    target: [
        "esnext"
    ],
    platform: "browser",
    sourcemap: true,
    format: "esm",
    globalName: "CkbLightClient",
    minify: profile === "prod",
    logLevel: "debug"
})
