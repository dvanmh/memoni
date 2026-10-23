#include <fcitx-utils/dbus/bus.h>
#include <fcitx-utils/dbus/objectvtable.h>
#include <fcitx-utils/rect.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <cmath>
#include <fcitx/inputcontext.h>
#include <fcitx/instance.h>
#include <dbus_public.h>
#include <stdexcept>

namespace fcitx {

class CursorRect : public AddonInstance,
                   public dbus::ObjectVTable<CursorRect> {
    FCITX_ADDON_DEPENDENCY_LOADER(dbus, instance_->addonManager());

public:
    explicit CursorRect(Instance *instance) : instance_(instance) {
        auto *bus = dbus()->call<IDBusModule::bus>();
        if (!bus || !bus->addObjectVTable("/cursorrect", "org.fcitx.Fcitx.CursorRect", *this)) {
            throw std::runtime_error("failed to register D-Bus object");
        }
    }

    std::tuple<int, int, int, int> getCursorRect() {
        auto *ic = instance_->lastFocusedInputContext();
        if (!ic) {
            throw dbus::MethodCallError("org.fcitx.Fcitx.CursorRect.Error.NoFocus", "No focused input context");
        }
        const Rect &r = ic->cursorRect();
        const double scale = ic->scaleFactor();

        // Probably a XIM client with only XNSpotLocation provided, we'll use a reasonable default
        if (r.height() == 0) {
            const double height = 30.0;
            return {
                static_cast<int>(std::lround(r.left() * scale)),
                static_cast<int>(std::lround((r.top() - height) * scale)),
                static_cast<int>(std::lround(r.width() * scale)),
                static_cast<int>(std::lround(height * scale))
            };
        }

        return {
            static_cast<int>(std::lround(r.left() * scale)),
            static_cast<int>(std::lround(r.top() * scale)),
            static_cast<int>(std::lround(r.width() * scale)),
            static_cast<int>(std::lround(r.height() * scale))
        };
    }

private:
    FCITX_OBJECT_VTABLE_METHOD(getCursorRect, "GetCursorRect", "", "iiii");

    Instance *instance_;
};

class CursorRectFactory : public AddonFactory {
    AddonInstance *create(AddonManager *manager) override {
        return new CursorRect(manager->instance());
    }
};

} // namespace fcitx

FCITX_ADDON_FACTORY(fcitx::CursorRectFactory)
