import Foundation
import Speech
import AVFoundation

/// Speech recognition result callback
public typealias SpeechResultCallback = @convention(c) (UnsafePointer<CChar>?, UnsafeMutableRawPointer?) -> Void

/// Global speech recognizer instance
private var globalRecognizer: SpeechRecognizer?

/// Initialize the speech recognizer
@_cdecl("macos_speech_init")
public func macOSSpeechInit() -> Int32 {
    if globalRecognizer == nil {
        globalRecognizer = SpeechRecognizer()
    }
    return globalRecognizer?.requestPermissions() ?? 0
}

/// Recognize speech from audio buffer
@_cdecl("macos_speech_recognize")
public func macOSSpeechRecognize(
    audioData: UnsafePointer<Float>,
    audioLength: Int,
    sampleRate: Int32,
    callback: @escaping SpeechResultCallback,
    userData: UnsafeMutableRawPointer?
) {
    guard let recognizer = globalRecognizer else {
        let error = "Speech recognizer not initialized".cString(using: .utf8)!
        error.withUnsafeBufferPointer { ptr in
            callback(ptr.baseAddress, userData)
        }
        return
    }

    // Convert audio data to AVAudioPCMBuffer
    guard let audioBuffer = createAudioBuffer(
        from: audioData,
        length: audioLength,
        sampleRate: Double(sampleRate)
    ) else {
        let error = "Failed to create audio buffer".cString(using: .utf8)!
        error.withUnsafeBufferPointer { ptr in
            callback(ptr.baseAddress, userData)
        }
        return
    }

    // Perform recognition
    recognizer.recognize(audioBuffer: audioBuffer) { result, error in
        if let error = error {
            let errorMsg = error.localizedDescription.cString(using: .utf8)!
            errorMsg.withUnsafeBufferPointer { ptr in
                callback(ptr.baseAddress, userData)
            }
        } else if let transcript = result {
            let transcriptCStr = transcript.cString(using: .utf8)!
            transcriptCStr.withUnsafeBufferPointer { ptr in
                callback(ptr.baseAddress, userData)
            }
        } else {
            let empty = "".cString(using: .utf8)!
            empty.withUnsafeBufferPointer { ptr in
                callback(ptr.baseAddress, userData)
            }
        }
    }
}

/// Check if speech recognition is available
@_cdecl("macos_speech_available")
public func macOSSpeechAvailable() -> Int32 {
    return SFSpeechRecognizer.authorizationStatus() == .authorized ? 1 : 0
}

// MARK: - Helper Classes

private class SpeechRecognizer {
    private let recognizer: SFSpeechRecognizer
    private var recognitionRequest: SFSpeechAudioBufferRecognitionRequest?
    private var recognitionTask: SFSpeechRecognitionTask?

    init() {
        recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))!
    }

    func requestPermissions() -> Int32 {
        var authStatus = SFSpeechRecognizer.authorizationStatus()

        if authStatus == .notDetermined {
            let semaphore = DispatchSemaphore(value: 0)
            SFSpeechRecognizer.requestAuthorization { status in
                authStatus = status
                semaphore.signal()
            }
            semaphore.wait()
        }

        return authStatus == .authorized ? 1 : 0
    }

    func recognize(audioBuffer: AVAudioPCMBuffer, completion: @escaping (String?, Error?) -> Void) {
        // Cancel any in-flight recognition task before starting a new one.
        // Without this, rapid calls stack up XPC connections and macOS cancels them all.
        recognitionTask?.cancel()
        recognitionTask = nil
        recognitionRequest?.endAudio()
        recognitionRequest = nil

        recognitionRequest = SFSpeechAudioBufferRecognitionRequest()

        guard let recognitionRequest = recognitionRequest else {
            completion(nil, NSError(domain: "SpeechRecognizer", code: -1, userInfo: [NSLocalizedDescriptionKey: "Unable to create request"]))
            return
        }

        // Append audio buffer
        recognitionRequest.append(audioBuffer)
        recognitionRequest.endAudio()

        recognitionTask = recognizer.recognitionTask(with: recognitionRequest) { result, error in
            if let error = error {
                completion(nil, error)
            } else if let result = result, result.isFinal {
                completion(result.bestTranscription.formattedString, nil)
            }
        }
    }
}

// MARK: - Audio Buffer Creation

private func createAudioBuffer(from data: UnsafePointer<Float>, length: Int, sampleRate: Double) -> AVAudioPCMBuffer? {
    let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: sampleRate,
        channels: 1,
        interleaved: false
    )!

    let frameCapacity = AVAudioFrameCount(length)
    guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCapacity) else {
        return nil
    }

    buffer.frameLength = frameCapacity

    // Copy audio data
    if let channelData = buffer.floatChannelData {
        memcpy(channelData[0], data, length * MemoryLayout<Float>.size)
    }

    return buffer
}
