const path = require('path');
const webpack = require('webpack');

module.exports = {
    entry: './src/index.ts',
    target: ['web', 'es5'],
    output: {
        path: path.resolve(__dirname, 'dist'),
        filename: 'index.js',
        library: {
            type: "umd",
            name: "light-client-js",
            umdNamedDefine: true
        },
        globalObject: "globalThis",
        iife: true,
        publicPath: "auto"
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
        fallback: {
            stream: require.resolve("stream-browserify"),
        }
    },
    mode: "production",
    devtool: "source-map"
};
