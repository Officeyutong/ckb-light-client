const path = require('path');
const webpack = require('webpack');

module.exports = {
    entry: './src/index.ts',
    output: {
        path: path.resolve(__dirname, 'dist'),
        filename: 'index.js',
        library: {
            type: "module",
        },
        globalObject: "globalThis"
    },
    module: {
        rules: [
            {
                test: /\.ts$/,
                use: 'ts-loader',
                exclude: /node_modules/,
            },
        ],
    },
    resolve: {
        extensions: ['.ts', '.js'],
        fallback: { "stream": require.resolve("stream-browserify") }
    },
    mode: "production",
    experiments: {
        outputModule: true
    },
    devtool: "source-map"
};
