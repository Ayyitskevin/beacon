import {
  Button,
  Checkbox,
  Field,
  makeStyles,
  Select,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useState } from "react";

import type {
  BeaconCapability,
  BeaconCapabilityDescriptor,
  LocalModelActivation,
  ResidentAgentView,
  ResidentConfiguration,
  StarterRole,
  TeamIneligibilityReason,
} from "./session";

const roleLabels: Record<StarterRole, string> = {
  lead: "Lead",
  builder: "Builder",
  reviewer: "Reviewer",
  researcher: "Researcher",
};

const roleSuggestions: Record<StarterRole, readonly BeaconCapability[]> = {
  lead: [
    "work.create_mission",
    "work.form_team",
    "work.coordinate_mission",
    "work.assign_work",
  ],
  builder: ["artifact.produce"],
  reviewer: ["artifact.evaluate"],
  researcher: ["evidence.develop"],
};

const reasonLabels: Record<TeamIneligibilityReason, string> = {
  configuration_cleared: "configuration cleared",
  resident_inactive: "Resident inactive",
  resident_suspended: "Resident suspended",
  resident_retired: "Resident retired",
  model_activation_unavailable: "bound model activation unavailable",
};

const useStyles = makeStyles({
  panel: {
    marginTop: tokens.spacingVerticalXXL,
    ...shorthands.padding(tokens.spacingVerticalL, 0),
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    columnGap: tokens.spacingHorizontalL,
  },
  heading: {
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
    fontSize: tokens.fontSizeBase500,
  },
  description: {
    marginTop: 0,
    color: tokens.colorNeutralForeground2,
  },
  list: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    paddingLeft: 0,
    listStyleType: "none",
  },
  resident: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 1fr) auto",
    columnGap: tokens.spacingHorizontalL,
    rowGap: tokens.spacingVerticalS,
    alignItems: "start",
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground3,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  residentName: { margin: 0, fontSize: tokens.fontSizeBase400 },
  status: { margin: 0, color: tokens.colorNeutralForeground2 },
  actions: {
    display: "flex",
    flexWrap: "wrap",
    gap: tokens.spacingHorizontalS,
  },
  editor: {
    gridColumn: "1 / -1",
    display: "grid",
    rowGap: tokens.spacingVerticalM,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  capabilityList: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    margin: 0,
    padding: 0,
    ...shorthands.border(0),
  },
  capability: { display: "grid" },
  capabilityDescription: {
    marginLeft: tokens.spacingHorizontalXXL,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
  },
  boundary: {
    margin: 0,
    ...shorthands.padding(tokens.spacingVerticalS, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground4,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  confirmation: {
    marginTop: tokens.spacingVerticalM,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorPaletteRedBorder1),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
});

export function ResidentConfigurations({
  residents,
  catalog,
  configurations,
  activations,
  busy,
  loading,
  onLoad,
  onConfigure,
  onClear,
}: {
  residents: readonly ResidentAgentView[];
  catalog: readonly BeaconCapabilityDescriptor[] | null;
  configurations: readonly ResidentConfiguration[] | null;
  activations: readonly LocalModelActivation[] | null;
  busy: boolean;
  loading: boolean;
  onLoad: () => void;
  onConfigure: (
    residentId: string,
    expectedDecisionId: string | null,
    activationId: string,
    capabilities: readonly BeaconCapability[],
  ) => void;
  onClear: (residentId: string, expectedDecisionId: string) => void;
}) {
  const classes = useStyles();
  const [editingResidentId, setEditingResidentId] = useState<string | null>(null);
  const [selectedActivationId, setSelectedActivationId] = useState("");
  const [selectedCapabilities, setSelectedCapabilities] = useState<BeaconCapability[]>([]);
  const [clearCandidateId, setClearCandidateId] = useState<string | null>(null);
  const ready = catalog !== null && configurations !== null && activations !== null;
  const compatibleActivations = (activations ?? []).filter(
    (activation) =>
      activation.state === "active" && activation.approvedFeatures.includes("completion"),
  );
  const clearCandidate = clearCandidateId
    ? residents.find((resident) => resident.id === clearCandidateId) ?? null
    : null;
  const clearCandidateConfiguration = clearCandidate
    ? configurations?.find(
        (configuration) =>
          configuration.residentId === clearCandidate.id && configuration.state === "configured",
      ) ?? null
    : null;

  function beginEditing(resident: ResidentAgentView, configuration?: ResidentConfiguration) {
    const binding = configuration?.state === "configured" ? configuration.binding : null;
    const existingActivation = binding
      ? compatibleActivations.find((activation) => activation.id === binding.activationId)
      : null;
    setSelectedActivationId(existingActivation?.id ?? compatibleActivations[0]?.id ?? "");
    setSelectedCapabilities(
      binding ? [...binding.capabilities] : [...roleSuggestions[resident.role]],
    );
    setEditingResidentId(resident.id);
  }

  function submitConfiguration(
    event: FormEvent,
    resident: ResidentAgentView,
    configuration?: ResidentConfiguration,
  ) {
    event.preventDefault();
    if (!selectedActivationId || selectedCapabilities.length === 0) return;
    onConfigure(
      resident.id,
      configuration?.id ?? null,
      selectedActivationId,
      selectedCapabilities,
    );
    setEditingResidentId(null);
  }

  return (
    <section className={classes.panel} aria-labelledby="resident-configuration-heading">
      <div className={classes.header}>
        <div>
          <h2 id="resident-configuration-heading" className={classes.heading}>
            Resident configuration
          </h2>
          <p className={classes.description}>
            Bind each Resident to one approved local-model activation and an explicit Capability
            set. This makes a Resident eligible for Team formation; it does not grant execution
            authority.
          </p>
        </div>
        <Button size="small" disabled={busy || loading} onClick={onLoad}>
          {ready ? "Refresh configurations" : "Review Resident configurations"}
        </Button>
      </div>
      {loading ? (
        <p role="status">Loading Resident configurations</p>
      ) : !ready ? (
        <p className={classes.status}>Not reviewed yet</p>
      ) : residents.length === 0 ? (
        <p className={classes.status}>No Residents are available to configure.</p>
      ) : (
        <ul className={classes.list} aria-label="Resident configurations">
          {residents.map((resident) => {
            const label = resident.displayName ?? roleLabels[resident.role];
            const configuration = configurations.find(
              (item) => item.residentId === resident.id,
            );
            const configured = configuration?.state === "configured";
            const canEdit = resident.lifecycle !== "retired" && compatibleActivations.length > 0;
            return (
              <li key={resident.id} className={classes.resident}>
                <div>
                  <h3 className={classes.residentName}>{label}</h3>
                  <p className={classes.status}>{configurationStatus(configuration)}</p>
                </div>
                <div className={classes.actions}>
                  {resident.lifecycle !== "retired" && (
                    <Button
                      size="small"
                      disabled={busy || !canEdit}
                      onClick={() => beginEditing(resident, configuration)}
                    >
                      {configured ? `Edit ${label}` : `Configure ${label}`}
                    </Button>
                  )}
                  {configured && (
                    <Button
                      size="small"
                      appearance="subtle"
                      disabled={busy}
                      onClick={() => setClearCandidateId(resident.id)}
                    >
                      Clear configuration for {label}
                    </Button>
                  )}
                </div>
                {!canEdit && resident.lifecycle !== "retired" && (
                  <p className={classes.status}>
                    Activate a local model with completion approved before configuring this
                    Resident.
                  </p>
                )}
                {editingResidentId === resident.id && (
                  <form
                    className={classes.editor}
                    onSubmit={(event) => submitConfiguration(event, resident, configuration)}
                  >
                    <p className={classes.status}>
                      These role-based suggestions are not authority. Review and confirm every
                      Capability.
                    </p>
                    <Field label={`Local model for ${label}`}>
                      <Select
                        aria-label={`Local model for ${label}`}
                        value={selectedActivationId}
                        onChange={(_, data) => setSelectedActivationId(data.value)}
                      >
                        {compatibleActivations.map((activation) => (
                          <option key={activation.id} value={activation.id}>
                            {activation.binding.modelName}
                          </option>
                        ))}
                      </Select>
                    </Field>
                    <fieldset className={classes.capabilityList}>
                      <legend>Capabilities for {label}</legend>
                      {catalog.map((descriptor) => (
                        <div key={descriptor.capability} className={classes.capability}>
                          <Checkbox
                            label={descriptor.label}
                            checked={selectedCapabilities.includes(descriptor.capability)}
                            onChange={(_, data) =>
                              setSelectedCapabilities(
                                data.checked
                                  ? catalog
                                      .map((item) => item.capability)
                                      .filter(
                                        (capability) =>
                                          capability === descriptor.capability ||
                                          selectedCapabilities.includes(capability),
                                      )
                                  : selectedCapabilities.filter(
                                      (capability) => capability !== descriptor.capability,
                                    ),
                              )
                            }
                          />
                          <span className={classes.capabilityDescription}>
                            {descriptor.description}
                          </span>
                        </div>
                      ))}
                    </fieldset>
                    <p className={classes.boundary}>
                      Budget boundary: <strong>No spend</strong>. Configuration cannot authorize
                      purchases, external effects, or model execution.
                    </p>
                    <div className={classes.actions}>
                      <Button
                        type="submit"
                        appearance="primary"
                        disabled={
                          busy || !selectedActivationId || selectedCapabilities.length === 0
                        }
                      >
                        Save Resident configuration
                      </Button>
                      <Button disabled={busy} onClick={() => setEditingResidentId(null)}>
                        Cancel
                      </Button>
                    </div>
                  </form>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {clearCandidate && clearCandidateConfiguration && (
        <section
          className={classes.confirmation}
          role="alertdialog"
          aria-labelledby="clear-resident-configuration-heading"
          aria-describedby="clear-resident-configuration-description"
        >
          <h3 id="clear-resident-configuration-heading">
            Clear this Resident configuration?
          </h3>
          <p id="clear-resident-configuration-description">
            {clearCandidate.displayName ?? roleLabels[clearCandidate.role]} will immediately become
            ineligible for new Team formation. The decision history remains auditable.
          </p>
          <div className={classes.actions}>
            <Button
              appearance="primary"
              disabled={busy}
              onClick={() => {
                onClear(clearCandidate.id, clearCandidateConfiguration.id);
                setClearCandidateId(null);
              }}
            >
              Clear Resident configuration
            </Button>
            <Button disabled={busy} onClick={() => setClearCandidateId(null)}>
              Cancel
            </Button>
          </div>
        </section>
      )}
    </section>
  );
}

function configurationStatus(configuration: ResidentConfiguration | undefined): string {
  if (!configuration) return "Not configured; not eligible for Team formation";
  if (configuration.state === "cleared") {
    return "Cleared; not eligible for Team formation";
  }
  if (configuration.eligibleForTeam) return "Eligible for Team formation";
  return `Configured; not eligible for Team formation (${configuration.ineligibilityReasons
    .map((reason) => reasonLabels[reason])
    .join(", ")})`;
}
