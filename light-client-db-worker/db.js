self.onerror = event => {
    console.error(event);
}


self.onmessage = async (evt) => {
    const data = evt.data;
    const wasmModule = await import(data.entryJsPath);
    await wasmModule.default({ module_or_path: data.wasmModulePath });
    wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
    self.postMessage({});
    await wasmModule.main_loop();
}
