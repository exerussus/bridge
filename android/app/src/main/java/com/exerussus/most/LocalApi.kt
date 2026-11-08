package com.exerussus.most

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/** Ошибка локального API: текст — для человека (мост отвечает по-русски). */
class ApiException(val status: Int, message: String) : Exception(message)

/**
 * Локальный API узла — тот же, что у панели ПК: `/bridge/v1/local/…`, заголовок `X-Bridge-Local`.
 * Только 127.0.0.1, поэтому без прокси.
 */
class LocalApi(private val base: String, private val token: String) {
    suspend fun get(path: String): JSONObject = call("GET", path, null)

    suspend fun post(path: String, body: JSONObject = JSONObject()): JSONObject = call("POST", path, body)

    private suspend fun call(method: String, path: String, body: JSONObject?): JSONObject = withContext(Dispatchers.IO) {
        val rb = body?.toString()?.toRequestBody(JSON)
        val req = Request.Builder()
            .url("$base/bridge/v1/local/${path.trimStart('/')}")
            .header("X-Bridge-Local", token)
            .method(method, rb)
            .build()
        http.newCall(req).execute().use { r ->
            val text = r.body?.string().orEmpty()
            if (!r.isSuccessful) {
                val msg = runCatching { JSONObject(text).optString("error") }.getOrNull().orEmpty()
                throw ApiException(r.code, msg.ifEmpty { "мост ответил ${r.code}" })
            }
            if (text.isBlank()) JSONObject() else JSONObject(text)
        }
    }

    companion object {
        private val JSON = "application/json; charset=utf-8".toMediaType()
        private val http = OkHttpClient.Builder()
            .proxy(java.net.Proxy.NO_PROXY)
            .connectTimeout(5, TimeUnit.SECONDS)
            .readTimeout(130, TimeUnit.SECONDS)
            .build()
    }
}
