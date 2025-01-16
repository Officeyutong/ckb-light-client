const path = require('path');
const WasmPackPlugin = require("@wasm-tool/wasm-pack-plugin");
const webpack = require("webpack");
module.exports = {
    entry: './index.ts',
    target: "webworker",
    output: {
        path: path.resolve(__dirname, 'dist'),
        filename: 'index.js',
        library: {
            type: "module",
        },
        publicPath: "/",
        chunkFormat: false
    },
    plugins: [
        new WasmPackPlugin({
            crateDirectory: path.resolve(__dirname, "."),
            outName: "light-client-wasm",
            extraArgs: "--target web"
        }),
        new webpack.ProvidePlugin({
            Buffer: ['buffer', 'Buffer'],
        }),
    ],
    module: {
        rules: [
            {
                test: /\.wasm$/,
                loader: "arraybuffer-loader"
            },
            {
                test: /\.ts$/,
                use: 'ts-loader',
                exclude: /node_modules/,
            }
        ]

    },
    experiments: {
        outputModule: true
    },
    mode: "production",
    resolve: {
        fallback: {
            buffer: require.resolve('buffer/'),
        },
    },
};