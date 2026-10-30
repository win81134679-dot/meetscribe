package io.github.win81134679.meetscribe

import android.app.Application
import io.github.win81134679.meetscribe.core.Repo

class MeetScribeApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Repo.init(this)
    }
}
