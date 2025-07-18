<!-- Copyright 2023 OpenObserve Inc.

 Licensed under the Apache License, Version 2.0 (the "License");
 you may not use this file except in compliance with the License.
 You may obtain a copy of the License at

     http:www.apache.org/licenses/LICENSE-2.0

 Unless required by applicable law or agreed to in writing, software
 distributed under the License is distributed on an "AS IS" BASIS,
 WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 See the License for the specific language governing permissions and
 limitations under the License. 
-->

<!-- eslint-disable vue/no-unused-components -->
<template>
  <div
    style="padding: 0px 10px; min-width: 30%"
    class="scroll o2-input"
    data-test="dashboard-drilldown-popup"
  >
    <div
      class="flex justify-between items-center q-pa-md"
      style="border-bottom: 2px solid gray; margin-bottom: 5px"
    >
      <div class="flex items-center q-table__title q-mr-md">
        <span data-test="dashboard-drilldown-title" v-if="isEditMode"
          >Edit Drilldown
        </span>
        <span data-test="dashboard-drilldown-title" v-else
          >Create Drilldown</span
        >
      </div>
      <div class="flex q-gutter-sm items-center">
        <DrilldownUserGuide />
      </div>
    </div>
    <q-input
      v-model="drilldownData.name"
      :label="t('dashboard.nameOfVariable') + ' * ' + ' : '"
      color="input-border"
      bg-color="input-bg"
      class="q-py-md showLabelOnTop"
      stack-label
      outlined
      filled
      data-test="dashboard-config-panel-drilldown-name"
      dense
      :rules="[(val: any) => !!val.trim() || t('dashboard.nameRequired')]"
      :lazy-rules="true"
    />
    <div
      style="display: flex; flex-direction: row; gap: 10px; align-items: center"
    >
      Go to :
      <q-btn-group class="">
        <q-btn
          :class="drilldownData.type == 'byDashboard' ? 'selected' : ''"
          size="sm"
          @click="changeTypeOfDrilldown('byDashboard')"
          data-test="dashboard-drilldown-by-dashboard-btn"
          ><q-icon
            class="q-mr-xs"
            :name="outlinedDashboard"
            style="cursor: pointer; height: 25px"
          />Dashboard</q-btn
        >
        <q-btn
          :class="drilldownData.type === 'byUrl' ? 'selected' : ''"
          size="sm"
          @click="changeTypeOfDrilldown('byUrl')"
          data-test="dashboard-drilldown-by-url-btn"
          ><q-icon
            class="q-mr-xs"
            name="link"
            style="cursor: pointer; height: 25px; display: flex !important"
          />URL</q-btn
        >
        <q-btn
          :class="drilldownData.type === 'logs' ? 'selected' : ''"
          size="sm"
          @click="changeTypeOfDrilldown('logs')"
          data-test="dashboard-drilldown-by-logs-btn"
          ><q-icon
            class="q-mr-xs"
            name="search"
            style="cursor: pointer; height: 25px; display: flex !important"
          />Logs</q-btn
        >
      </q-btn-group>
    </div>

    <div v-if="drilldownData.type === 'logs'" style="margin-top: 10px">
      <div>
        <label>Select Logs Mode:</label>
        <q-btn-group class="q-ml-sm">
          <q-btn
            :class="drilldownData.data.logsMode === 'auto' ? 'selected' : ''"
            size="sm"
            @click="drilldownData.data.logsMode = 'auto'"
          >
            Auto
          </q-btn>
          <q-btn
            :class="drilldownData.data.logsMode === 'custom' ? 'selected' : ''"
            size="sm"
            @click="drilldownData.data.logsMode = 'custom'"
          >
            Custom
          </q-btn>
        </q-btn-group>
      </div>
      <div v-if="drilldownData.data.logsMode === 'custom'" style="margin-top: 10px">
        <label>Enter Custom Query:</label>
        <query-editor
          data-test="scheduled-alert-sql-editor"
          ref="queryEditorRef"
          editor-id="alerts-query-editor"
          class="monaco-editor"
          style="height: 80px"
          :debounceTime="300"
          v-model:query="drilldownData.data.logsQuery"
          @update:query="updateQueryValue"
        />
      </div>
    </div>
    <div v-if="drilldownData.type == 'byUrl'">
      <div style="margin-top: 10px; display: flex; flex-direction: column">
        Enter URL:
        <textarea
          style="
            min-width: 100%;
            max-width: 100%;
            resize: vertical;
            border: 1px solid;
            border-radius: 4px;
            padding: 2px;
          "
          v-model="drilldownData.data.url"
          :class="store.state.theme == 'dark' ? 'dark-mode' : 'bg-white'"
          data-test="dashboard-drilldown-url-textarea"
        ></textarea>
        <div
          style="color: red; font-size: 12px"
          v-if="!isFormURLValid && drilldownData.data.url.trim()"
          data-test="dashboard-drilldown-url-error-message"
        >
          Invalid URL
        </div>
      </div>
    </div>

    <div v-if="drilldownData.type == 'byDashboard'">
      <div style="margin-top: 10px">
        <div class="dropdownDiv">
          <q-select
            v-model="drilldownData.data.folder"
            :options="folderList"
            emit-value
            label="Select Folder * :"
            color="input-border"
            bg-color="input-bg"
            class="q-py-sm showLabelOnTop no-case"
            stack-label
            outlined
            filled
            dense
            style="width: 100%"
            :loading="getFoldersListLoading.isLoading.value"
            :disable="getFoldersListLoading.isLoading.value"
            data-test="dashboard-drilldown-folder-select"
          >
            <!-- template when on options -->
            <template v-slot:no-option>
              <q-item>
                <q-item-section class="text-italic text-grey">
                  No folders available
                </q-item-section>
              </q-item>
            </template>
          </q-select>
        </div>
        <div class="dropdownDiv" v-if="drilldownData.data.folder">
          <q-select
            v-model="drilldownData.data.dashboard"
            :options="dashboardList"
            emit-value
            label="Select Dashboard * :"
            color="input-border"
            bg-color="input-bg"
            class="q-py-sm showLabelOnTop no-case"
            stack-label
            outlined
            filled
            dense
            style="width: 100%"
            :loading="getDashboardListLoading.isLoading.value"
            :disable="getDashboardListLoading.isLoading.value"
            data-test="dashboard-drilldown-dashboard-select"
          >
            <!-- template when on options -->
            <template v-slot:no-option>
              <q-item
                data-test="dashboard-drilldown-no-dashboard-available-option"
              >
                <q-item-section class="text-italic text-grey">
                  No dashboards available
                </q-item-section>
              </q-item>
            </template>
          </q-select>
        </div>
        <div class="dropdownDiv" v-if="drilldownData.data.dashboard">
          <q-select
            v-model="drilldownData.data.tab"
            :options="tabList"
            emit-value
            label="Select Tab * :"
            color="input-border"
            bg-color="input-bg"
            class="q-py-sm showLabelOnTop no-case"
            stack-label
            outlined
            filled
            dense
            style="width: 100%"
            :loading="getTabListLoading.isLoading.value"
            :disable="getTabListLoading.isLoading.value"
            data-test="dashboard-drilldown-tab-select"
          >
            <!-- template when on options -->
            <template v-slot:no-option>
              <q-item data-test="dashboard-drilldown-no-tab-available-option">
                <q-item-section class="text-italic text-grey">
                  No tab Available
                </q-item-section>
              </q-item>
            </template>
          </q-select>
        </div>

        <!-- array of variables name and its values -->
        <div style="margin-top: 30px">
          <div
            style="
              display: flex;
              justify-content: space-between;
              margin-bottom: 10px;
              align-items: center;
            "
          >
            <div>Variables :</div>
            <q-btn
              icon="add"
              color="primary"
              size="sm"
              padding="sm"
              @click="
                () =>
                  drilldownData.data.variables.push({
                    name: '',
                    value: '',
                  })
              "
              data-test="dashboard-drilldown-add-variable"
              >Add</q-btn
            >
          </div>
          <div
            v-for="(variable, index) in drilldownData.data.variables"
            :key="index"
          >
            <div
              style="display: flex; gap: 10px; margin-bottom: 10px"
              :key="JSON.stringify(variableNamesFn ?? {})"
            >
              <CommonAutoComplete
                placeholder="Name"
                v-model="variable.name"
                searchRegex="(.*)"
                :items="variableNamesFn"
                style="width: auto !important; padding-top: 3px !important"
              ></CommonAutoComplete>
              <CommonAutoComplete
                placeholder="Value"
                searchRegex="(.*)"
                v-model="variable.value"
                :items="options.selectedValue"
                style="width: auto !important; padding-top: 3px !important"
              ></CommonAutoComplete>

              <q-icon
                class="q-mr-xs"
                size="20px"
                name="close"
                style="cursor: pointer; height: 54px; display: flex !important"
                @click="() => drilldownData.data.variables.splice(index, 1)"
                :data-test="`dashboard-drilldown-variable-remove-${index}`"
              />
            </div>
          </div>
        </div>
      </div>
      <!-- radio button for new tab -->
      <div style="margin-top: 10px">
        <q-toggle
          :label="`Pass all current variables : `"
          left-label
          v-model="drilldownData.data.passAllVariables"
          data-test="dashboard-drilldown-pass-all-variables"
        />
      </div>
    </div>

    <!-- radio button for new tab -->
    <div style="margin-top: 10px">
      <q-toggle
        :label="`Open in new tab : `"
        left-label
        v-model="drilldownData.targetBlank"
        data-test="dashboard-drilldown-open-in-new-tab"
      />
    </div>

    <q-card-actions class="confirmActions">
      <q-btn
        unelevated
        no-caps
        class="q-mr-sm"
        @click="$emit('close')"
        data-test="cancel-button"
      >
        {{ t("confirmDialog.cancel") }}
      </q-btn>
      <q-btn
        unelevated
        no-caps
        class="no-border"
        color="primary"
        @click="saveDrilldown"
        style="min-width: 60px"
        data-test="confirm-button"
        :disable="isFormValid"
        :label="isEditMode ? 'Update' : 'Add'"
      />
    </q-card-actions>
  </div>
</template>

<script lang="ts">
import { defineAsyncComponent, inject, reactive, ref } from "vue";
import { defineComponent } from "vue";
import { useI18n } from "vue-i18n";
import {
  outlinedDashboard,
  outlinedDelete,
} from "@quasar/extras/material-icons-outlined";
import { watch } from "vue";
import { useStore } from "vuex";
import { computed } from "vue";
import {
  getAllDashboardsByFolderId,
  getDashboard,
  getFoldersList,
} from "../../../utils/commons";
import { onMounted, onUnmounted } from "vue";
import useDashboardPanelData from "../../../composables/useDashboardPanel";
import DrilldownUserGuide from "@/components/dashboards/addPanel/DrilldownUserGuide.vue";
import CommonAutoComplete from "@/components/dashboards/addPanel/CommonAutoComplete.vue";
import { useLoading } from "@/composables/useLoading";
const QueryEditor = defineAsyncComponent(
  () => import("@/components/CodeQueryEditor.vue"),
);

export default defineComponent({
  name: "DrilldownPopUp",
  components: {
    DrilldownUserGuide,
    CommonAutoComplete,
    QueryEditor,
  },
  props: {
    isEditMode: {
      type: Boolean,
      default: false,
    },
    drilldownDataIndex: {
      type: Number,
      default: -1,
    },
    variablesData: {
      type: Object,
      default: () => {
        return {};
      },
    },
  },
  emits: ["close"],
  setup(props, { emit }) {
    const { t } = useI18n();
    const store = useStore();
    const dashboardPanelDataPageKey = inject(
      "dashboardPanelDataPageKey",
      "dashboard"
    );
    const { dashboardPanelData } = useDashboardPanelData(
      dashboardPanelDataPageKey
    );
    
    const getDefaultDrilldownData = () => ({
      name: "",
      type: "byDashboard",
      targetBlank: false,
      findBy: "name",
      data: {
        logsMode: "auto",
        logsQuery: "",
        url: "",
        folder: "",
        dashboard: "",
        tab: "",
        passAllVariables: true,
        variables: [
          {
            name: "",
            value: "",
          },
        ],
      },
    });
    const drilldownData = ref(
      props?.isEditMode
        ? JSON.parse(
            JSON.stringify(
              dashboardPanelData.data.config.drilldown[
                props?.drilldownDataIndex
              ]
            )
          )
        : getDefaultDrilldownData()
    );
    const dashboardList: any = ref([]);
    const tabList: any = ref([]);

    const getFoldersListLoading = useLoading(async () => {
      await getFoldersList(store);
    });

    const getDashboardListLoading = useLoading(async () => {
      await getDashboardList();
    });

    const getTabListLoading = useLoading(async () => {
      await getTabList();
    });

    onMounted(async () => {
      // if no folders in organization, get folders
      if (
        !store.state.organizationData.folders ||
        (Array.isArray(store.state.organizationData.folders) &&
          store.state.organizationData.folders.length === 0)
      ) {
        // get folders(will be api call)
        await getFoldersListLoading.execute();
      }

      // get dashboard list
      // get tab list
      await getDashboardListLoading.execute();
      await getTabListLoading.execute();

      // get variables list
      await getvariableNames();
    });

    // on folder change, reset dashboard and tab values
    watch(
      () => drilldownData.value.data.folder,
      async (newVal, oldVal) => {
        await getDashboardListLoading.execute();
        if (newVal !== oldVal) {
          // take first value from new options list
          drilldownData.value.data.dashboard =
            dashboardList?.value[0]?.value ?? "";
          drilldownData.value.data.tab = tabList?.value[0]?.value ?? "";
        }
      }
    );

    // on dashboard change, reset tab value
    watch(
      () => drilldownData.value.data.dashboard,
      async (newVal, oldVal) => {
        await getTabListLoading.execute();
        if (newVal !== oldVal) {
          // take first value from new options list
          drilldownData.value.data.tab = tabList?.value[0]?.value ?? "";
        }
      }
    );

    const folderList = computed(() => {
      // if no folders in organization, return []
      if (!store.state.organizationData.folders) {
        return [];
      }
      // make list of options from folders list
      return (
        store.state.organizationData.folders?.map((folder: any) => {
          return {
            label: folder.name,
            value: folder.name,
          };
        }) ?? []
      );
    });

    const getDashboardList = async () => {
      // get folder data
      // by using folder name, find folder data
      const folderData = store.state.organizationData.folders?.find(
        (folder: any) => folder.name === drilldownData.value.data.folder
      );

      // if no folder with same forder name found, return
      if (!folderData) {
        dashboardList.value = [];
        return;
      }

      // get all dashboards from folder
      const allDashboardList = await getAllDashboardsByFolderId(
        store,
        folderData?.folderId
      );

      // make list of dashboards
      dashboardList.value =
        allDashboardList?.map((dashboard: any) => {
          return {
            label: dashboard.title,
            value: dashboard.title,
          };
        }) ?? [];
    };

    const getTabList = async () => {
      // get folder data
      // by using folder name, find folder data      
      const folderData = store.state.organizationData.folders?.find(
        (folder: any) => folder.name === drilldownData.value.data.folder
      );

      // if no folder with same forder name found, return
      if (!folderData) {
        dashboardList.value = [];
        return;
      }
      // want dashboardId from dashboard name
      // by using dashboard name, find dashboard data      
      // get all dashboards from folder
      const allDashboardList = await getAllDashboardsByFolderId(
        store,
        folderData?.folderId
      );
      
      // get dashboardId from allDashboardList by dashboard name
      const dashboardId = allDashboardList?.find(
        (dashboard: any) =>
          dashboard.title === drilldownData.value.data.dashboard
      )?.dashboardId;
            
      if (!dashboardId) {  
        tabList.value = [];  
        return;  
      }  

      // get dashboard data
      // by using dashboard name, find dashboard data
      const dashboardData = await getDashboard(store, dashboardId, folderData?.folderId);
      
      // if no dashboard with same dashboard name found, return
      if (!dashboardData) {
        tabList.value = [];
        return;
      }

      // make list of tabs
      tabList.value =
        dashboardData?.tabs?.map((tab: any) => {
          return {
            label: tab.name,
            value: tab.name,
          };
        }) ?? [];
    };

    const isFormURLValid = computed(() => {
      // check if url is valid with protocol only(will check only protocol)
      const urlRegex = /^(http|https|ftp|file|mailto|telnet|data|ws|wss):\/\//;
      return urlRegex.test(drilldownData.value.data.url.trim());
    });

    const isFormValid = computed(() => {
      // if name is empty
      if (!drilldownData.value.name.trim()) {
        return true;
      }

      // if action is not selected
      if (!drilldownData.value.type) {
        return true;
      }

      // if action is by url
      if (drilldownData.value.type == "byUrl") {
        if (drilldownData.value.data.url.trim()) {
          // check if url is valid with protocol
          return !isFormURLValid.value;
        }
      } else if (drilldownData.value.type == "logs") {
        if (drilldownData.value.data.logsMode === "custom") {
          return !drilldownData.value.data.logsQuery.trim();
        } else if (drilldownData.value.data.logsMode === "auto") {          
          return false;
        }
      } else {
        if (
          drilldownData.value.data.folder &&
          drilldownData.value.data.dashboard &&
          drilldownData.value.data.tab
        ) {
          return false;
        }
      }
      return true;
    });

    const saveDrilldown = () => {
      // if editmode then made changes
      // else add new drilldown
      if (props?.isEditMode) {        
        dashboardPanelData.data.config.drilldown[props?.drilldownDataIndex] =
          drilldownData.value;
      } else {        
        dashboardPanelData.data.config.drilldown.push(drilldownData.value);
      }
      emit("close");
    };

    // change type of drilldown
    const changeTypeOfDrilldown = (type: string) => {
      drilldownData.value.type = type;
    };

    const options: any = reactive({
      selectedValue: [],
      selectedName: [],
    });

    //want label for dropdown in input and value for its input value
    const selectedValue = computed(() => {
      let selectedValues: any = [];
      const variableListName =
        props?.variablesData?.values
          ?.filter((variable: any) => variable.type !== "dynamic_filters")
          ?.map((variable: any) => ({
            label: variable.name,
            value: "${" + variable.name + "}",
          })) ?? [];

      if (dashboardPanelData.data.type === "sankey") {
        selectedValues = [
          { label: "Edge Source", value: "${edge.__source}" },
          { label: "Edge Target", value: "${edge.__target}" },
          { label: "Edge Value", value: "${edge.__value}" },
          { label: "Node Name", value: "${node.__name}" },
          { label: "Node Value", value: "${node.__value}" },
          ...variableListName,
        ];
      } else if (dashboardPanelData.data.type === "table") {
        dashboardPanelData.data.queries.forEach((query: any) => {
          const panelFields = [
            ...query.fields.x,
            ...query.fields.y,
            ...query.fields.z,
            ...variableListName,
          ];
          panelFields.forEach((field) => {
            selectedValues.push({
              label: field.label,
              value: '${row.field["' + field.label + '"]}',
            });
          });
        });
      } else if (
        ["pie", "donut", "gauge"].includes(dashboardPanelData.data.type)
      ) {
        selectedValues = [
          { label: "Series Name", value: "${series.__name}" },
          { label: "Series Value", value: "${series.__value}" },
          ...variableListName,
        ];
      } else if (dashboardPanelData.data.type === "metric") {
        selectedValues = [
          { label: "Series Value", value: "${series.__value}" },
          ...variableListName,
        ];
      } else {
        selectedValues = [
          { label: "Series Name", value: "${series.__name}" },
          { label: "Series Value", value: "${series.__value}" },
          { label: "Axis Value", value: "${series.__axisValue}" },
          ...variableListName,
        ];
      }
      return selectedValues;
    });

    // Assign selectedValue to options object
    options.selectedValue = selectedValue;

    const variableNamesFn = ref([]);

    const getvariableNames = async () => {      
      if (
        drilldownData.value.data.folder &&
        drilldownData.value.data.dashboard
      ) {
        const folder = store.state.organizationData.folders.find(
          (folder: any) => folder.name === drilldownData.value.data.folder
        );

        const allDashboardData = await getAllDashboardsByFolderId(
          store,
          folder.folderId
        );

        const dashboardId = allDashboardData?.find(
        (dashboard: any) =>
          dashboard.title === drilldownData.value.data.dashboard
      )?.dashboardId;
      
      if (!dashboardId) {  
        variableNamesFn.value = [];  
        return;  
      } 
        const dashboardData = await getDashboard(store, dashboardId, folder?.folderId);

        if (dashboardData) {
          const optionsList = dashboardData.variables.list.map(
            (variable: any) => ({
              label: variable.name,
              value: variable.name,
            })
          );
          variableNamesFn.value = optionsList;
        } else {
          variableNamesFn.value = [];
        }
      }
    };

    watch(drilldownData.value, async (newData) => {
      if (newData.data.folder && newData.data.dashboard) {
        await getvariableNames();
      } else {
        variableNamesFn.value = [];
      }
    });

    const updateQueryValue = (value: string) => {
      drilldownData.value.data.logsQuery = value;
    };

    return {
      t,
      outlinedDashboard,
      dashboardPanelData,
      drilldownData,
      outlinedDelete,
      store,
      folderList,
      dashboardList,
      tabList,
      isFormValid,
      saveDrilldown,
      isFormURLValid,
      changeTypeOfDrilldown,
      options,
      variableNamesFn,
      updateQueryValue,
      getFoldersListLoading,
      getDashboardListLoading,
      getTabListLoading,
    };
  },
});
</script>

<style lang="scss" scoped>
.selected {
  background-color: var(--q-primary) !important;
  font-weight: bold;
  color: white;
}
.dropdownDiv {
  display: flex;
  align-items: center;
  margin: 10px 0px;
  width: 100%;
}

.dropdownLabel {
  width: 150px;
}

.dropdown {
  min-width: 100%;
}

:deep(.no-case .q-field__native > :first-child) {
  text-transform: none !important;
}
</style>
