onerror = err => {
    console.error(err);
}
let wasmModule;
onmessage = async (evt) => {
    const data = evt.data;
    if (wasmModule === undefined) {
        wasmModule = await import(data.entryJsPath);
        await wasmModule.default(data.wasmModulePath);
        wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
        wasmModule.light_client(data.netName);
        self.postMessage({});
        return;
    }
    self.postMessage(wasmModule[evt.data.name](...evt.data.args))
};
